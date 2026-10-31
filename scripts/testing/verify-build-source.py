#!/usr/bin/env python3
"""Verify linked build provenance through a real Cargo Git dependency."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace-parent", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--expect-marker-failure", action="store_true")
    parser.add_argument("--source-revision", help="Read build scripts from an immutable local Git revision")
    args = parser.parse_args()
    evidence = args.evidence.resolve()
    evidence.mkdir(parents=True, exist_ok=False)
    workspace = Path(tempfile.mkdtemp(prefix="build-source-", dir=args.workspace_parent))
    environment = os.environ.copy()
    for key in ("UC_ENGINE_SOURCE_COMMIT", "UC_ENGINE_SOURCE_STATE", "MBX_STATS_REPORT", "MBX_BYPASS_LOG"):
        environment.pop(key, None)
    environment.update(
        CARGO_HOME=str(workspace / "cargo-home"),
        CARGO_TARGET_DIR=str(workspace / "consumer/target"),
        MBX_CACHE_DIR=str(workspace / "mbx-cache"),
        UC_ENGINE_MBX_STATS_DIR=str(evidence / "mbx"),
        UC_ENGINE_MBX_LOCAL_ONLY="1",
        GIT_CONFIG_GLOBAL=os.devnull,
        GIT_CONFIG_NOSYSTEM="1",
        CARGO_INCREMENTAL="0",
    )
    results = []
    receipt = {"passed": False, "cases": results, "workspace": str(workspace)}

    def command(argv, *, cwd=ROOT, env=None, input=None):
        result = subprocess.run(argv, cwd=cwd, env=env or environment,
                                input=input, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        with (evidence / "commands.log").open("a") as output:
            output.write(json.dumps({"argv": [str(a) for a in argv], "cwd": str(cwd),
                                     "returncode": result.returncode}) + "\n")
            output.write(result.stdout + result.stderr + "\n")
        result.check_returncode()
        return result.stdout.strip()

    def git(root, *argv):
        return command(["git", "-C", str(root), *argv])

    def initialize(root):
        root.mkdir()
        git(root, "init", "-q")
        git(root, "config", "user.name", "Build source verification")
        git(root, "config", "user.email", "verification@example.invalid")

    def commit(root, message):
        git(root, "add", ".")
        git(root, "-c", "user.name=Build source verification", "-c",
            "user.email=verification@example.invalid", "commit", "-qm", message)
        return git(root, "rev-parse", "HEAD")

    def add_submodule(root, source, relative):
        git(root, "-c", "protocol.file.allow=always", "submodule", "add", "-q",
            source.as_uri(), relative)

    try:
        receipt["engine_commit"] = git(ROOT, "rev-parse", "HEAD")
        receipt["engine_status"] = git(ROOT, "status", "--porcelain")
        receipt["source_hashes"] = {}
        source_revision = git(ROOT, "rev-parse", "--verify", args.source_revision + "^{commit}") if args.source_revision else None
        receipt["source_revision"] = source_revision
        receipt["rustc"] = command(["rustc", "--version"])
        receipt["cargo"] = command(["cargo", "--version"])
        leaf = workspace / "leaf source"
        initialize(leaf)
        (leaf / "lib.rs").write_text("pub fn leaf() {}\n")
        commit(leaf, "Leaf fixture")
        iroh = workspace / "iroh source"
        initialize(iroh)
        (iroh / "lib.rs").write_text("pub fn original() {}\n")
        add_submodule(iroh, leaf, "nested child")
        commit(iroh, "Submodule fixture")
        engine = workspace / "engine source"
        initialize(engine)
        package = engine / "crates/provenance-source"
        (package / "src").mkdir(parents=True)
        (engine / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/provenance-source"]\nresolver = "2"\n')
        (package / "Cargo.toml").write_text(
            '[package]\nname = "provenance-source"\nversion = "0.1.0"\nedition = "2021"\n'
            '[build-dependencies]\ncc = "=1.2.62"\n')
        for name in ("build.rs", "build_support.rs"):
            original = ROOT / "crates/uc-observability-runtime" / name
            content = subprocess.check_output(["git", "-C", str(ROOT), "show",
                                              source_revision + ":crates/uc-observability-runtime/" + name]) if source_revision else original.read_bytes()
            (package / name).write_bytes(content)
            receipt["source_hashes"][name] = hashlib.sha256(content).hexdigest()
        (package / "src/lib.rs").write_text(
            'extern "C" { static uc_observability_build_source_v1: [u8; 64]; }\n'
            'pub fn source() -> String {\n'
            ' let bytes = unsafe { &uc_observability_build_source_v1 };\n'
            ' String::from_utf8_lossy(bytes).trim_end_matches(char::from(0)).to_string()\n}\n')
        add_submodule(engine, iroh, "third_party/iroh")
        engine_sha = commit(engine, "Engine build-script fixture")
        consumer = workspace / "consumer"
        (consumer / "src").mkdir(parents=True)
        (consumer / "Cargo.toml").write_text(
            '[package]\nname = "provenance-consumer"\nversion = "0.1.0"\nedition = "2021"\n'
            '[dependencies]\nprovenance-source = { git = ' + json.dumps(engine.as_uri())
            + ', rev = ' + json.dumps(engine_sha) + ' }\n')
        (consumer / "src/main.rs").write_text('fn main() { println!("{}", provenance_source::source()); }\n')
        cargo = ["cargo", "--manifest-path", str(consumer / "Cargo.toml")]
        # Cargo subcommands precede --manifest-path.
        def cargo_command(subcommand, *extra, env=None):
            return command([cargo[0], subcommand, *cargo[1:], *extra], env=env)
        cargo_command("fetch")
        checkouts = list((workspace / "cargo-home/git/checkouts").glob("*/*/crates/provenance-source"))
        if len(checkouts) != 1:
            raise RuntimeError("Expected one Cargo Git checkout")
        checkout = checkouts[0].parents[1]
        sub = checkout / "third_party/iroh"
        nested = sub / "nested child"
        markers = [checkout / ".cargo-ok", sub / ".cargo-ok", nested / ".cargo-ok"]
        receipt["cargo_markers"] = [dict(relative=str(p.relative_to(checkout)),
                                        size=p.stat().st_size,
                                        sha256=hashlib.sha256(p.read_bytes()).hexdigest()) for p in markers]
        if not all(p.is_file() and p.stat().st_size == 0 for p in markers):
            raise RuntimeError("Cargo did not create the expected empty checkout markers")
        binary = consumer / "target/debug/provenance-consumer"
        if os.name == "nt":
            binary = binary.with_suffix(".exe")
        def check(name, expected, *, env=None):
            cargo_command("clean", "-p", "provenance-source")
            # Each case must execute the build script, including injected Git failures.
            # Keep the repository MBX wrapper, with a fresh owned cache per case.
            case_environment = (env or environment) | {"MBX_CACHE_DIR": str(workspace / ("mbx-" + name))}
            cargo_command("build", "--offline", "--locked", env=case_environment)
            observed = command([str(binary)])
            actual_sha, actual_state = observed.split()
            row = dict(name=name, expected=expected, observed=actual_state,
                       source_commit=actual_sha, binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                       engine_status=git(checkout, "status", "--porcelain"),
                       submodule_status=git(sub, "status", "--porcelain", "--untracked-files=all"))
            results.append(row)
            if actual_state != expected or actual_sha != ("unknown" if name == "git_failure" else engine_sha):
                if args.expect_marker_failure and name == "cargo_generated_markers" and actual_state == "modified":
                    receipt["expected_baseline_failure"] = True
                    return False
                raise AssertionError(f"{name}: expected {expected}, got {observed}")
            return True
        if not check("cargo_generated_markers", "clean"):
            return
        if args.expect_marker_failure:
            raise AssertionError("Baseline unexpectedly accepted the Cargo markers")
        for marker in markers:
            marker.unlink()
        check("markers_removed", "clean")
        for marker in markers:
            marker.touch()
        check("markers_restored", "clean")
        git_directory = git(checkout, "rev-parse", "--absolute-git-dir")
        hook_environment = environment | {
            "GIT_DIR": git_directory,
            "GIT_WORK_TREE": str(checkout),
            "GIT_INDEX_FILE": str(Path(git_directory) / "index"),
            "GIT_PREFIX": "crates/provenance-source/",
        }
        check("inherited_git_repository", "clean", env=hook_environment)
        alternate_index_environment = environment | {
            "GIT_INDEX_FILE": str(workspace / "unrelated-index"),
        }
        check("inherited_git_index", "clean", env=alternate_index_environment)
        (sub / "lib.rs").write_text("pub fn changed() {}\n")
        check("inherited_git_repository_with_edit", "modified", env=hook_environment)
        check("marker_and_tracked_edit", "modified")
        git(sub, "add", "lib.rs")
        check("marker_and_staged_edit", "modified")
        git(sub, "reset", "--hard", "HEAD")
        unusual = sub / 'new source λ\n".rs'
        unusual.write_text("pub fn added() {}\n")
        check("marker_and_untracked_source", "modified")
        unusual.unlink()
        (nested / "lib.rs").write_text("pub fn changed_leaf() {}\n")
        check("nested_submodule_edit", "modified")
        git(nested, "restore", "lib.rs")
        (sub / ".cargo-ok").write_text("not a Cargo checkout marker\n")
        check("nonempty_marker", "modified")
        (sub / ".cargo-ok").write_bytes(b"")
        git(sub, "add", ".cargo-ok")
        check("tracked_marker", "modified")
        git(sub, "reset", "HEAD", "--", ".cargo-ok")
        (sub / ".cargo-ok").unlink()
        (sub / ".cargo-ok").symlink_to("lib.rs")
        check("symlink_marker", "modified")
        (sub / ".cargo-ok").unlink()
        (sub / ".cargo-ok").touch()
        (sub / ".cargo-ok").unlink()
        (sub / ".cargo-ok").mkdir()
        (sub / ".cargo-ok/source.rs").write_text("pub fn marker_directory() {}\n")
        check("marker_directory_with_source", "modified")
        shutil.rmtree(sub / ".cargo-ok")
        (sub / ".cargo-ok").touch()
        (sub / "arbitrary").mkdir()
        (sub / "arbitrary/.cargo-ok").touch()
        check("marker_at_arbitrary_path", "modified")
        shutil.rmtree(sub / "arbitrary")
        (sub / "lib.rs").write_text("pub fn next_revision() {}\n")
        commit(sub, "Changed submodule revision")
        check("submodule_revision", "modified")
        git(checkout, "add", "third_party/iroh")
        check("staged_submodule_revision", "modified")
        git(checkout, "reset", "HEAD", "--", "third_party/iroh")
        git(sub, "reset", "--hard", git(iroh, "rev-parse", "HEAD"))
        git(checkout, "config", "submodule.third_party/iroh.ignore", "all")
        (sub / "lib.rs").write_text("pub fn cannot_hide() {}\n")
        check("git_config_cannot_hide_edit", "modified")
        git(sub, "restore", "lib.rs")
        git(checkout, "config", "--unset", "submodule.third_party/iroh.ignore")
        (sub / "lib.rs").unlink()
        check("deleted_source", "modified")
        git(sub, "restore", "lib.rs")
        git(sub, "mv", "lib.rs", "renamed source.rs")
        check("renamed_source", "modified")
        git(sub, "reset", "--hard", "HEAD")
        blob = git(sub, "rev-parse", "HEAD:lib.rs")
        conflict = "0 " + "0" * 40 + "\tlib.rs\n" + "".join(f"100644 {blob} {stage}\tlib.rs\n" for stage in (1, 2, 3))
        command(["git", "-C", str(sub), "update-index", "--index-info"], input=conflict)
        check("source_conflict", "modified")
        git(sub, "reset", "--hard", "HEAD")
        hidden = workspace / "hidden-submodule"
        sub.rename(hidden)
        sub.mkdir()
        check("missing_submodule", "unknown")
        sub.rmdir()
        hidden.rename(sub)
        tools = workspace / "git-failure-tools"
        tools.mkdir()
        shim = tools / "git"
        shim.write_text('#!/bin/sh\nexit 19\n')
        shim.chmod(0o755)
        failed_git_environment = environment | {"PATH": str(tools) + os.pathsep + environment["PATH"]}
        check("git_failure", "unknown", env=failed_git_environment)
        real_git = shutil.which("git", path=environment["PATH"])
        shim.write_text('#!/bin/sh\nfor arg in "$@"; do\n if [ "$arg" = status ]; then exit 19; fi\ndone\nexec ' + shlex.quote(real_git) + ' "$@"\n')
        check("git_status_failure", "unknown", env=failed_git_environment)
        shim.write_text('#!/bin/sh\ncase "$PWD" in */third_party/iroh) exit 19;; esac\nexec ' + shlex.quote(real_git) + ' "$@"\n')
        check("git_submodule_failure", "unknown", env=failed_git_environment)
        check("restored_checkout", "clean")
        receipt["fixture_commit"] = engine_sha
        receipt["cargo_lock_sha256"] = hashlib.sha256((consumer / "Cargo.lock").read_bytes()).hexdigest()
        if not source_revision and receipt["source_hashes"] != {name: hashlib.sha256((ROOT / "crates/uc-observability-runtime" / name).read_bytes()).hexdigest() for name in receipt["source_hashes"]}:
            raise RuntimeError("Engine build source changed during verification")
        receipt["passed"] = True
    except BaseException as error:
        receipt["passed"] = False
        receipt["error_type"] = type(error).__name__
        raise
    finally:
        # Every child command is awaited. Reclaim only this invocation's owned workspace.
        shutil.rmtree(workspace)
        receipt["owned_workspace_removed"] = not workspace.exists()
        (evidence / "result.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({"passed": receipt["passed"], "cases": len(results), "evidence": str(evidence)}))


if __name__ == "__main__":
    main()
