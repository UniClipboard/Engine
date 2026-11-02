#!/usr/bin/env python3
"""Run real Engine processes through three already-authorized Tailscale containers."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import secrets
import subprocess
import threading
import time


HOST_SHA = "8bdf1f5f15a05278ddaf5bf814d879aafd7b2d52e3a626c473e146f098a6f09d"
FIXTURE_SHA = "446c03d4726a80fed3f766209b6239ca9eaaa7bb5f96557c956fee1b5a8221c5"
CHAIN = "UC343TS"


def run(args, **kwargs):
    result = subprocess.run(args, capture_output=True, timeout=30, **kwargs)
    if result.returncode:
        raise RuntimeError("control_command_failed")
    return result.stdout


def until(check, seconds, reason):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            if time.monotonic() >= deadline:
                break
            return
        time.sleep(0.2)
    raise RuntimeError(reason)


class Host:
    def __init__(self, alias):
        self.alias = alias
        self.ts = "uc-pr343-ts-" + alias.lower()
        self.workload = "uc-pr343-ts-engine-" + alias.lower()
        self.process = None
        self.storage = None
        self.ready = False
        self.policy_installed = False

    def ts_exec(self, *args):
        return run(["docker", "exec", self.ts, *args])

    def start(self):
        self.responses = queue.Queue()
        self.process = subprocess.Popen(
            ["docker", "exec", "-i", "--user", "10001:10001", self.workload,
             "/opt/uc/integrated-final-normal"], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
            bufsize=1,
        )

        def read():
            for line in self.process.stdout:
                try:
                    value = json.loads(line)
                    if value.get("uc_connectivity") == 1:
                        self.responses.put(value)
                except ValueError:
                    pass
            self.responses.put(None)

        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()
        startup = {"root": "/profiles/test"}
        if self.storage is not None:
            startup["secure_storage"] = self.storage
        response = self.raw(startup)
        self.ready = bool(response.get("ready"))
        if not self.ready:
            raise RuntimeError("host_not_ready")
        if "production-network-policy" not in response.get("host", {}).get("capabilities", []):
            raise RuntimeError("production_policy_missing")

    def raw(self, request):
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        try:
            value = self.responses.get(timeout=120)
        except queue.Empty:
            raise RuntimeError("host_command_timeout") from None
        if value is None:
            raise RuntimeError("host_exited")
        return value

    def call(self, command, **fields):
        value = self.raw({"command": command, **fields})
        if value.get("error"):
            raise RuntimeError("host_operation_failed_" + command + "_" + str(value.get("code", "unknown")))
        return value.get("ok")

    def stop(self):
        if not self.process:
            return
        if self.process.poll() is not None:
            raise RuntimeError("host_exited_before_shutdown")
        self.storage = self.call("secure_storage")
        self.call("shutdown")
        self.process.stdin.close()
        if self.process.wait(timeout=15) != 0:
            raise RuntimeError("host_shutdown_failed")
        self.reader.join(timeout=2)
        self.process = None
        self.ready = False

    def namespace(self, *args, **kwargs):
        return run(["nsenter", "-t", str(self.pid), "-n", *args], **kwargs)

    def policy(self, hosts):
        existing = subprocess.run(
            ["nsenter", "-t", str(self.pid), "-n", "nft", "list", "table", "inet", CHAIN],
            capture_output=True, timeout=5,
        )
        if existing.returncode == 0:
            raise RuntimeError("existing_policy_table_must_be_preserved")
        addresses4 = ", ".join(p.ip for p in hosts if p != self)
        addresses6 = ", ".join(p.ip6 for p in hosts if p != self)
        rules = f'''table inet {CHAIN} {{
          chain output {{ type filter hook output priority -10; policy accept;
            meta skuid 10001 oifname "lo" counter accept
            meta skuid 10001 oifname "tailscale0" ip saddr {self.ip} ip daddr {{ {addresses4} }} meta l4proto udp counter accept comment "overlay4"
            meta skuid 10001 oifname "tailscale0" ip6 saddr {self.ip6} ip6 daddr {{ {addresses6} }} meta l4proto udp counter accept comment "overlay6"
            meta skuid 10001 counter drop comment "bypass_dropped"
          }}
          chain input {{ type filter hook input priority -10; policy accept;
            iifname "lo" accept
            iifname "tailscale0" ip saddr {{ {addresses4} }} ip daddr {self.ip} udp dport {self.port} counter accept
            iifname "tailscale0" ip6 saddr {{ {addresses6} }} ip6 daddr {self.ip6} udp dport {self.port} counter accept
            udp dport {self.port} counter drop comment "listener_bypass_dropped"
          }}
        }}\n'''
        self.namespace("nft", "-f", "-", input=rules.encode())
        self.policy_installed = True

    def counters(self):
        counts = {"overlay_packets": 0, "underlay_dropped": 0}
        rows = json.loads(self.namespace("nft", "-j", "list", "table", "inet", CHAIN))["nftables"]
        for row in rows:
            rule = row.get("rule", {})
            label = rule.get("comment")
            packets = sum(e.get("counter", {}).get("packets", 0) for e in rule.get("expr", []))
            if label in ["overlay4", "overlay6"]:
                counts["overlay_packets"] += packets
            if label == "bypass_dropped":
                counts["underlay_dropped"] += packets
        return counts

    def remove_policy(self):
        if self.policy_installed:
            self.namespace("nft", "delete", "table", "inet", CHAIN)
            self.policy_installed = False


def udp_preflight(hosts):
    rows = []
    for sender, receiver in zip(hosts, hosts[1:] + hosts[:1]):
        route = json.loads(sender.namespace("ip", "-j", "route", "get", receiver.ip))[0]
        if route.get("dev") != "tailscale0" or route.get("prefsrc") != sender.ip:
            raise RuntimeError("exact_overlay_route_missing")
        server_code = (
            "import socket; s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);"
            f"s.bind(({receiver.ip!r},{receiver.port}));s.settimeout(8);"
            "print('ready',flush=True);d,a=s.recvfrom(128);s.sendto(d,a)"
        )
        server = subprocess.Popen(["nsenter", "-t", str(receiver.pid), "-n", "python3", "-u", "-c", server_code],
                                  stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        ready = queue.Queue()
        threading.Thread(target=lambda: ready.put(server.stdout.readline().strip()), daemon=True).start()
        try:
            if ready.get(timeout=3) != "ready":
                raise RuntimeError("udp_control_server_not_ready")
            client_code = (
                "import socket; s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);s.settimeout(5);"
                f"s.sendto(b'uc-ts-route-control',({receiver.ip!r},{receiver.port}));"
                "assert s.recvfrom(128)[0]==b'uc-ts-route-control'"
            )
            sender.namespace("setpriv", "--reuid=10001", "--regid=10001", "--clear-groups", "python3", "-c", client_code)
            if server.wait(timeout=3) != 0:
                raise RuntimeError("udp_control_server_failed")
            before = sender.counters()["underlay_dropped"]
            meta = json.loads(run(["docker", "inspect", receiver.ts]))[0]
            underlay = next(n["IPAddress"] for n in meta["NetworkSettings"]["Networks"].values() if n["IPAddress"])
            negative = (
                "import socket; s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);"
                "\ntry:\n"
                f" s.sendto(b'uc-ts-negative',({underlay!r},{receiver.port}))\n"
                "except OSError as e:\n assert e.errno in (1,13)\n"
            )
            sender.namespace("setpriv", "--reuid=10001", "--regid=10001", "--clear-groups", "python3", "-c", negative)
            if sender.counters()["underlay_dropped"] <= before:
                raise RuntimeError("underlay_negative_control_not_hit")
            rows.append({"from": sender.alias, "to": receiver.alias, "exact_tail_route": True,
                         "same_uid_udp_roundtrip": True, "underlay_udp_drop_hit": True})
        finally:
            if server.poll() is None:
                server.kill()
                server.wait(timeout=3)
    return rows


def transfer(sender, receiver, files, facts, canaries):
    content = secrets.token_bytes(512 * 1024) if files else ("uc-ts-" + secrets.token_hex(24)).encode()
    if not files:
        canaries.append(content)
    expected = hashlib.sha256(content).hexdigest()
    fact = {"from": sender.alias, "to": receiver.alias, "kind": "file" if files else "text",
            "bytes": len(content), "sha256": expected, "receiver_matched": False}
    facts.append(fact)
    if files:
        sent = sender.call("send_file", peer=receiver.id, handle="synthetic-" + secrets.token_hex(8),
                           display_name="synthetic.bin", bytes=list(content))
    else:
        sent = sender.call("send", peer=receiver.id, text=content.decode())

    def readback():
        for entry in receiver.call("history"):
            if files and entry.get("content_type") != "file":
                continue
            row = receiver.raw({"command": "read_file" if files else "entry", "entry": entry["entry_id"]})
            if row.get("error"):
                continue
            value = row.get("ok") or {}
            data = bytes(value.get("bytes", [])) if files else value.get("content", "").encode()
            if hashlib.sha256(data).hexdigest() == expected:
                return True
        return False

    until(readback, 60 if files else 20, "receiver_readback_deadline")
    fact["receiver_matched"] = True

    def settled():
        view = sender.call("delivery", entry=sent["entry_id"])
        deliveries = view.get("deliveries", [])
        statuses = [item["status"]["kind"] for item in deliveries if item["target_device_id"] == receiver.id]
        fact["sender_statuses"] = statuses
        fact["other_target_statuses"] = [item["status"]["kind"] for item in deliveries if item["target_device_id"] != receiver.id]
        return len(statuses) == 1 and statuses[0] in ["delivered", "duplicate"]

    until(settled, 10, "sender_settlement_deadline")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--fixture", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    args = parser.parse_args()
    if args.evidence.exists():
        raise SystemExit("evidence_already_exists")
    args.evidence.parent.mkdir(parents=True, exist_ok=True)
    hosts = [Host(alias) for alias in "ABC"]
    facts = {"status": "failed", "topology": "three_real_tailnet_nodes_one_physical_vps",
             "host_sha256": HOST_SHA, "fixture_sha256": FIXTURE_SHA,
             "transfers": [], "cleanup": False, "privacy_scan": False}
    canaries = []
    started = time.monotonic()
    try:
        if hashlib.sha256(args.binary.read_bytes()).hexdigest() != HOST_SHA:
            raise RuntimeError("binary_identity_mismatch")
        if hashlib.sha256(args.fixture.read_bytes()).hexdigest() != FIXTURE_SHA:
            raise RuntimeError("fixture_identity_mismatch")
        suffixes = []
        for index, host in enumerate(hosts):
            status = json.loads(host.ts_exec("tailscale", "status", "--json"))
            if status.get("BackendState") != "Running":
                raise RuntimeError("tailscale_not_authorized")
            suffixes.append(status.get("CurrentTailnet", {}).get("MagicDNSSuffix"))
            host.ip = next(ip for ip in status["TailscaleIPs"] if "." in ip)
            host.ip6 = next(ip for ip in status["TailscaleIPs"] if ":" in ip)
            host.port = 23110 + index
        if not suffixes[0] or len(set(suffixes)) != 1:
            raise RuntimeError("tailnet_mismatch")
        for index, host in enumerate(hosts):
            facts["phase"] = "preflight_" + host.alias
            meta = json.loads(run(["docker", "inspect", host.ts]))[0]
            worker = json.loads(run(["docker", "inspect", host.workload]))[0]
            if meta["Config"]["Labels"].get("uc.task") != "pr343-real-tailscale":
                raise RuntimeError("namespace_ownership_mismatch")
            if worker["Config"]["Labels"].get("uc.task") != "pr343-real-tailscale":
                raise RuntimeError("workload_ownership_mismatch")
            if worker["HostConfig"]["NetworkMode"] != "container:" + meta["Id"]:
                raise RuntimeError("workload_namespace_mismatch")
            for filename, expected in [("integrated-final-normal", HOST_SHA), ("invitation-fixture", FIXTURE_SHA)]:
                actual = run(["docker", "exec", host.workload, "sha256sum", "/opt/uc/" + filename]).decode().split()[0]
                if actual != expected:
                    raise RuntimeError("mounted_binary_identity_mismatch")
            host.pid = meta["State"]["Pid"]
            if os.stat("/proc/self/ns/net").st_ino == os.stat(f"/proc/{host.pid}/ns/net").st_ino:
                raise RuntimeError("host_namespace_not_isolated")
            facts["phase"] = "policy_" + host.alias
            host.policy(hosts)
        facts["phase"] = "udp_preflight"
        facts["route_controls"] = udp_preflight(hosts)
        for host in hosts:
            host.before_engine = host.counters()
            facts["phase"] = "bootstrap_" + host.alias
            host.start()
            result = host.call("settings_update", patch={"network": {
                "allow_relay_fallback": False, "allow_overlay_network_addrs": True,
                "bind_port": host.port}})
            if not result.get("Updated"):
                raise RuntimeError("network_settings_rejected")
            host.stop()
            host.start()
        facts["phase"] = "pairing"
        created = hosts[0].call("create", name="A")
        hosts[0].id = created["device"]
        for host in hosts[1:]:
            invitation = hosts[0].call("invite")["invitation"]
            canaries.append(invitation.encode())
            endpoint = hosts[0].ip + ":" + str(hosts[0].port)
            payload = {"invitation": invitation, "replace": True, "direct": [endpoint],
                       "contains_direct": [endpoint]}
            transformed = json.loads(run(["docker", "exec", "-i", hosts[0].workload,
                                          "/opt/uc/invitation-fixture"], input=json.dumps(payload).encode()))
            if not all(transformed.get(k) for k in ["endpoint_identity_preserved", "invitation_identity_preserved", "expiry_preserved"]):
                raise RuntimeError("invitation_identity_changed")
            if transformed["direct_count"] != 1 or transformed["relay_count"] != 0 or transformed["contains_direct"] != [True]:
                raise RuntimeError("invitation_not_overlay_only")
            canaries.append(transformed["invitation"].encode())
            host.call("join", invitation=transformed["invitation"], name=host.alias)
            def completed():
                response = host.raw({"command": "setup"})
                if response.get("code") in [1103, 1211]:
                    return False
                if response.get("error"):
                    raise RuntimeError("pairing_setup_query_failed")
                setup = response.get("ok") or {}
                return setup.get("has_completed") and setup.get("space_id") == created["space"]
            until(completed, 30, "pairing_deadline")
            def identify():
                peers = hosts[0].call("peers")
                host.id = next((p["peer_id"] for p in peers if p["device_name"] == host.alias), None)
                return bool(host.id)
            until(identify, 5, "joined_identity_missing")
            facts.setdefault("paired_nodes", []).append(host.alias)
        for host in hosts:
            overview = host.call("relay_overview")
            if overview["saved_mode"] != "Disabled" or overview["applied_mode"] != "Disabled" or overview["change_pending"]:
                raise RuntimeError("lan_only_not_applied")
            expected_peers = {p.id for p in hosts if p != host}
            def online():
                response = host.raw({"command": "peers"})
                if response.get("code") in [1103, 1211]:
                    return False
                if response.get("error"):
                    raise RuntimeError("peer_status_query_failed")
                return expected_peers.issubset({p["peer_id"] for p in response.get("ok", []) if p.get("connected")})
            until(online, 20, "three_node_online_deadline")
            def usable():
                response = host.raw({"command": "eligibility"})
                if response.get("code") in [1103, 1211]:
                    return False
                if response.get("error"):
                    raise RuntimeError("membership_eligibility_query_failed")
                devices = response.get("ok", {}).get("device_trust", {}).get("devices", [])
                return expected_peers.issubset({d["device_id"] for d in devices if d.get("sync_relationship") == "usable"})
            until(usable, 20, "three_node_membership_not_usable")
        facts["three_node_membership"] = True
        facts["phase"] = "transfers"
        for files in [False, True]:
            for sender in hosts:
                for receiver in hosts:
                    if sender != receiver:
                        facts["phase"] = "transfer_" + sender.alias + receiver.alias + ("_file" if files else "_text")
                        transfer(sender, receiver, files, facts["transfers"], canaries)
        facts["network_counters"] = {host.alias: host.counters() for host in hosts}
        for host in hosts:
            row = facts["network_counters"][host.alias]
            for key in row:
                row[key] -= host.before_engine[key]
        if not all(row["overlay_packets"] > 0 for row in facts["network_counters"].values()):
            raise RuntimeError("overlay_application_traffic_not_observed")
        facts["status"] = "passed"
    except Exception as error:
        facts["failure"] = str(error) if isinstance(error, RuntimeError) else type(error).__name__
    finally:
        clean = True
        privacy = True
        scanned = 0
        for host in hosts:
            try:
                host.stop()
            except Exception:
                clean = False
                if host.process:
                    try:
                        subprocess.run(["docker", "exec", "--user", "10001:10001", host.workload,
                                        "pkill", "-x", "integrated-fina"], capture_output=True, timeout=5)
                        host.process.kill()
                        host.process.wait(timeout=5)
                    except Exception:
                        clean = False
            try:
                listing = run(["docker", "exec", host.workload, "find", "/profiles", "-type", "f", "-print0"])
                for name in listing.decode().split("\0"):
                    if name:
                        data = run(["docker", "exec", host.workload, "cat", name])
                        scanned += 1
                        privacy = privacy and not any(c in data or (len(c) > 64 and c[len(c)//3:len(c)//3+32] in data) for c in canaries)
                run(["docker", "exec", host.workload, "rm", "-rf", "/profiles/test"])
                host.remove_policy()
            except Exception:
                clean = False
        facts.update(cleanup=clean, privacy_scan=privacy, privacy_files_scanned=scanned,
                     elapsed_seconds=round(time.monotonic() - started, 3))
        if not clean or not privacy or not scanned:
            facts["status"] = "failed"
        args.evidence.write_text(json.dumps(facts, indent=2) + "\n")
        print(json.dumps({k: v for k, v in facts.items() if k != "transfers"}))
    return 0 if facts["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
