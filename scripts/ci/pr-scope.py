"""PR 检查范围的唯一分类入口；任何不确定性均回退完整检查。"""
import json
import os
from pathlib import Path
import re
import subprocess

SNAPSHOT_FILES = {
    "scripts/build-cache/mbx-snapshot.py",
    "scripts/build-cache/mbx-snapshot.test.py",
}


def classify(raw):
    fields = raw.split(b"\0")
    if not raw or fields.pop() != b"" or len(fields) % 2:
        return "full"
    kinds = set()
    for header, filename in zip(fields[::2], fields[1::2]):
        match = re.fullmatch(rb":(\d{6}) (\d{6}) [0-9a-f]{40} [0-9a-f]{40} ([AMD])", header)
        if not match or any(mode not in (b"000000", b"100644") for mode in match.groups()[:2]):
            return "full"
        path = filename.decode("utf-8", errors="strict")
        if path in SNAPSHOT_FILES:
            kinds.add("snapshot")
        elif path == "docs/design-docs/observability.md":
            # Rust 合同测试直接嵌入该文档，不能按纯文字变更跳过。
            return "full"
        elif (path == "README.md" or path.startswith("docs/")) and path.endswith(".md") and not path.startswith("docs/generated/") and path.split("/")[-1] != "AGENTS.md":
            kinds.add("docs")
        else:
            return "full"
    return "snapshot" if "snapshot" in kinds else "docs"


def scope(event_name, event):
    if event_name != "pull_request":
        return "full"
    pr = event["pull_request"]
    if any(label["name"] == "full-ci" for label in pr["labels"]):
        return "full"
    base, head = pr["base"]["sha"], pr["head"]["sha"]
    if not all(re.fullmatch(r"[0-9a-f]{40}", sha) for sha in (base, head)):
        return "full"
    # 禁用重命名折叠：旧路径删除、新路径新增必须分别通过白名单。
    raw = subprocess.check_output(["git", "diff", "--raw", "--no-abbrev", "-z", "--no-renames", base + "..." + head])
    return classify(raw)


if __name__ == "__main__":
    try:
        result = scope(os.environ["GITHUB_EVENT_NAME"], json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text()))
    except (KeyError, ValueError, OSError, subprocess.CalledProcessError):
        result = "full"
    print("PR check scope: " + result)
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write("scope=" + result + "\n")
