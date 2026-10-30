#!/usr/bin/env python3
"""传输私有 CAS 快照；复用 Desktop 742bcfdc 的格式，闭包验证由 MBX 负责。"""

import argparse
import hashlib
import json
import os
import platform
import re
import sys
import shutil
import subprocess
import tarfile
import tempfile
import time
from pathlib import Path, PurePosixPath


BUCKET = "uniclipboard-build-cache"
PREFIX = "engine/mbx-snapshots/v1"
MANIFEST = "mbx-cache-export-v1.json"
MAX_ARCHIVE_BYTES = 16 * 1024**3
MAX_EXPANDED_BYTES = 40 * 1024**3
MAX_MEMBERS = 200_000
WINDOWS_DEVICES = {"CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"} | {
    f"{prefix}{digit}" for prefix in ("COM", "LPT") for digit in "123456789¹²³"
}
ROOT = Path(os.environ.get("UC_MBX_SNAPSHOT_SOURCE_ROOT", Path(__file__).resolve().parents[2])).resolve()

def digest_file(file):
    with open(file, "rb") as source:
        digest = hashlib.sha256()
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
        return digest.hexdigest()


def scene_key(scene):
    if not re.fullmatch(r"[a-zA-Z0-9_-]+", scene):
        raise ValueError("Invalid snapshot scene")
    digest = hashlib.sha256()
    names = ["Cargo.lock", "Cargo.toml", ".cargo/config.toml", "rust-toolchain.toml"]
    names += [str(p.relative_to(ROOT)) for p in sorted((ROOT / "scripts/build-cache").rglob("*"))
              if p.is_file() and p.suffix in (".sh", ".py")]
    names += [str(p.relative_to(ROOT)) for p in sorted((ROOT / ".github/workflows").glob("*.yml"))]
    names += [".github/actions/rust-ci-setup/action.yml"]
    for name in names:
        digest.update(name.encode() + b"\0" + (ROOT / name).read_bytes() + b"\0")
    digest.update(subprocess.check_output(["rustc", "-vV"]))
    # 场景限定目标、profile、SDK 和插桩策略；原生 MBX 仍逐动作验证所有输入。
    context = {k: os.environ.get(k, "") for k in (
        "RUNNER_OS", "RUNNER_ARCH", "ImageOS", "ImageVersion", "DEVELOPER_DIR",
        "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "UC_MBX_SNAPSHOT_VARIANT")}
    context.update(system=platform.system(), machine=platform.machine(), scene=scene)
    digest.update(json.dumps(context, sort_keys=True).encode())
    return f"{PREFIX}/{scene}/{digest.hexdigest()}"


def mbx(*args, cache=None):
    environment = {k: v for k, v in os.environ.items()
                   if not k.startswith(("BUILD_CACHE_R2_", "AWS_", "MBX_REMOTE_"))}
    environment["MBX_REMOTE_URL"] = ""
    environment["UC_ENGINE_MBX_LOCAL_ONLY"] = "1"
    if cache is not None:
        environment["MBX_CACHE_DIR"] = str(cache)
    # Windows 必须使用当前 Git Bash，避免 CreateProcess 误选 System32 的 WSL。
    bash = (subprocess.check_output(["cygpath", "-w", "/bin/bash"], text=True).strip()
            if os.name == "nt" else "bash")
    subprocess.run(
        [bash, "scripts/build-cache/mbx.sh", "--exec", "mbx", *args],
        cwd=ROOT, env=environment, check=True,
    )


def client():
    import boto3
    from botocore.config import Config

    return boto3.client(
        "s3", endpoint_url=os.environ["BUILD_CACHE_R2_ENDPOINT"],
        aws_access_key_id=os.environ["BUILD_CACHE_R2_ACCESS_KEY_ID"],
        aws_secret_access_key=os.environ["BUILD_CACHE_R2_SECRET_ACCESS_KEY"],
        region_name="auto",
        config=Config(connect_timeout=10, read_timeout=120,
                      s3={"addressing_style": "path"},
                      request_checksum_calculation="when_required",
                      response_checksum_validation="when_required",
                      retries={"total_max_attempts": 3, "mode": "standard"}),
    )


def get_index(s3, key):
    try:
        response = s3.get_object(Bucket=BUCKET, Key=f"{key}/latest.json")
    except Exception as error:
        from botocore.exceptions import ClientError

        if isinstance(error, ClientError) and error.response["Error"]["Code"] in ("NoSuchKey", "404"):
            return None
        raise
    with response["Body"] as body:
        raw = body.read(16 * 1024 + 1)
    if len(raw) > 16 * 1024:
        raise ValueError("Snapshot index exceeds limit")
    return validate_index(json.loads(raw), key)


def validate_index(value, key):
    if not isinstance(value, dict):
        raise ValueError("Invalid snapshot index")
    sha = value.get("sha256", "")
    size = value.get("size", 0)
    if (value.get("version") != 1 or not isinstance(sha, str) or len(sha) != 64
            or any(c not in "0123456789abcdef" for c in sha)
            or type(size) is not int or not 0 < size <= MAX_ARCHIVE_BYTES
            or value.get("key") != f"{key}/{sha}.tar.gz"):
        raise ValueError("Invalid snapshot index")
    if "source_sha" in value and not re.fullmatch(r"[0-9a-f]{40}", str(value["source_sha"])):
        raise ValueError("Invalid snapshot source")
    return value


def unpack(archive, destination):
    """仅接收 MBX 闭包常规文件，拒绝 target 状态及链接。"""
    total = 0
    seen = set()
    with tarfile.open(archive, "r:gz") as source:
        for entry in source:
            name = PurePosixPath(entry.name)
            if (name.is_absolute() or ".." in name.parts or entry.name in seen
                    or not name.parts or "\\" in entry.name
                    or any(any(c in part for c in ':<>"|?*') or part.endswith((".", " "))
                           or part.split(".")[0].upper() in WINDOWS_DEVICES
                           for part in name.parts)
                    or entry.size < 0 or not (entry.isfile() or entry.isdir())):
                raise ValueError("Unsafe snapshot member")
            seen.add(entry.name)
            if len(seen) > MAX_MEMBERS:
                raise ValueError("Too many snapshot members")
            if name.parts[0] not in (MANIFEST, "cas", "action-results"):
                raise ValueError("Unexpected snapshot member")
            total += entry.size
            if total > MAX_EXPANDED_BYTES:
                raise ValueError("Expanded snapshot exceeds limit")
            target = destination.joinpath(*name.parts)
            if entry.isdir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with source.extractfile(entry) as incoming, open(target, "xb") as output:
                    shutil.copyfileobj(incoming, output)
    manifest = json.loads((destination / MANIFEST).read_text())
    if manifest.get("attachments") or manifest.get("objects"):
        raise ValueError("Snapshot must not carry Cargo workspace state")


def compact_bundle(exported, destination):
    """剔除可选附件，再由原生 import 选择并验证闭包。"""
    manifest = json.loads((exported / MANIFEST).read_text())
    if manifest.get("version") not in (1, 2):
        raise ValueError("Unsupported MBX export format")
    manifest.pop("attachments", None)
    manifest.pop("objects", None)
    manifest["version"] = 1
    encoded = json.dumps(manifest).encode()
    (exported / MANIFEST).write_bytes(encoded)
    isolated = destination.parent / "isolated-cache"
    isolated.mkdir()
    # 原生 import 验证全部必需 blob、丢弃未引用附件并消费临时导出目录。
    mbx("cache", "import", str(exported), cache=isolated)
    destination.mkdir()
    (destination / MANIFEST).write_bytes(encoded)
    for name in ("cas", "action-results"):
        source = isolated / "actions" / name
        if source.exists():
            shutil.move(str(source), destination / name)


def restore(s3, key, temporary, report):
    report["index_reads"] += 1
    index = get_index(s3, key)
    if index is None:
        report["status"] = "miss"
        return
    archive = temporary / "snapshot.tar.gz"
    report["archive_reads"] += 1
    response = s3.get_object(Bucket=BUCKET, Key=index["key"])
    if response["ContentLength"] != index["size"]:
        response["Body"].close()
        raise ValueError("Snapshot size mismatch")
    with response["Body"] as body, open(archive, "wb") as output:
        while chunk := body.read(1024 * 1024):
            report["downloaded_bytes"] += len(chunk)
            if report["downloaded_bytes"] > index["size"]:
                raise ValueError("Snapshot exceeds declared size")
            output.write(chunk)
    if archive.stat().st_size != index["size"] or digest_file(archive) != index["sha256"]:
        raise ValueError("Snapshot checksum mismatch")
    bundle = temporary / "bundle"
    bundle.mkdir()
    unpack(archive, bundle)
    mbx("cache", "import", str(bundle))
    report.update(status="hit", source_sha=index.get("source_sha"))


def save(s3, key, temporary, report, scene, *, source_sha=None):
    if (os.environ.get("GITHUB_REF") != "refs/heads/main"
            or os.environ.get("GITHUB_REF_PROTECTED") != "true"
            or os.environ.get("GITHUB_EVENT_NAME") not in ("push", "schedule")):
        raise ValueError("Snapshot publication requires trusted protected main")
    if os.environ.get("UC_MBX_SNAPSHOT_WRITER") != "true":
        raise ValueError("Snapshot publication requires the designated writer")
    exported = temporary / "exported"
    mbx("cache", "export", "--group", scene, "--format", "directory", str(exported))
    bundle = temporary / "bundle"
    compact_bundle(exported, bundle)
    archive = temporary / "snapshot.tar.gz"
    with tarfile.open(archive, "w:gz", compresslevel=1) as output:
        for member in sorted(bundle.iterdir()):
            output.add(member, arcname=member.name)
    size = archive.stat().st_size
    if not 0 < size <= MAX_ARCHIVE_BYTES:
        raise ValueError("Snapshot archive exceeds limit")
    sha = digest_file(archive)
    index = {"version": 1, "sha256": sha, "size": size,
             "key": f"{key}/{sha}.tar.gz", "source_sha": source_sha or os.environ["GITHUB_SHA"]}
    # 完整归档先于索引发布；消费者永远不读取半成品。
    from boto3.s3.transfer import TransferConfig

    # 大归档使用 64 MiB 分片，减少 R2 A 类请求；限制并发，避免扩大上传资源占用。
    transfer = TransferConfig(multipart_threshold=64 * 1024**2,
                              multipart_chunksize=64 * 1024**2,
                              max_concurrency=4,
                              preferred_transfer_client="classic")
    s3.upload_file(str(archive), BUCKET, index["key"], Config=transfer)
    s3.put_object(Bucket=BUCKET, Key=f"{key}/latest.json",
                  Body=json.dumps(index).encode(), ContentType="application/json")
    report.update(status="saved", uploaded_bytes=size, source_sha=index["source_sha"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("restore", "save"))
    parser.add_argument("--scene", required=True)
    parser.add_argument("--require-hit", action="store_true")
    args = parser.parse_args()
    evidence = Path(os.environ["UC_ENGINE_MBX_STATS_DIR"])
    evidence.mkdir(parents=True, exist_ok=True)
    report = dict(operation=args.command, scene=args.scene, status="pending",
                  index_reads=0, archive_reads=0, downloaded_bytes=0, object_fallback=False)
    started = time.monotonic()
    try:
        key = os.environ.get("UC_MBX_SNAPSHOT_KEY") or scene_key(args.scene)
        if not re.fullmatch(re.escape(f"{PREFIX}/{args.scene}/") + r"[0-9a-f]{64}", key) or not re.fullmatch(r"[a-zA-Z0-9_-]+", args.scene):
            raise ValueError("Snapshot scene mismatch")
        if args.command == "restore" and os.environ.get("GITHUB_ENV"):
            with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as output:
                output.write(f"UC_MBX_SNAPSHOT_KEY={key}\nMBX_CACHE_EXPORT_GROUP={args.scene}\n")
        credentials = [os.environ.get("BUILD_CACHE_R2_" + name, "") for name in
                       ("ENDPOINT", "ACCESS_KEY_ID", "SECRET_ACCESS_KEY")]
        if not any(credentials):
            report["status"] = "no-credentials"
        else:
            if not all(credentials) or not credentials[0].startswith("https://"):
                raise ValueError("Invalid snapshot credentials")
            source_sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
            if args.command == "save" and source_sha != os.environ.get("GITHUB_SHA"):
                raise ValueError("Publisher must use the trusted workflow source")
            with tempfile.TemporaryDirectory(prefix="mbx-snapshot-", dir=os.environ["RUNNER_TEMP"]) as directory:
                s3 = client()
                if args.command == "restore":
                    restore(s3, key, Path(directory), report)
                else:
                    save(s3, key, Path(directory), report, args.scene, source_sha=source_sha)
    except Exception as error:
        # SDK 异常可能包含签名 URL；仅记录固定类型。失败后仍只能本地编译。
        report.update(status="error", error_type=type(error).__name__)
        print(f"::warning::MBX snapshot {args.command} failed ({type(error).__name__}); local cache only")
    report["duration_seconds"] = round(time.monotonic() - started, 3)
    (evidence / f"snapshot-{args.command}.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))
    if args.require_hit and report["status"] != "hit":
        return 1
    if args.command == "save" and report["status"] != "saved":
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
