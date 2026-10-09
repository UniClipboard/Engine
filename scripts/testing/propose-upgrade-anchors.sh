#!/usr/bin/env bash
# 发现新的 Desktop 公开发布后，提出追加升级兼容矩阵锚点的 PR。
#
# 用法（CI 中由 .github/workflows/upgrade-matrix-anchors.yml 调用）：
#   bash scripts/testing/propose-upgrade-anchors.sh [--dry-run]
#
# 需要已认证的 gh（CI 中为 GH_TOKEN）与可按 SHA 补取 Engine rev 的 origin；只被 PR 引用可达的 rev 由解析脚本补取。
# - 已有打开的锚点 PR 时在其分支上追加提交，保留审阅者补充的宿主能力等改动；否则从 main 重新开分支。
# - 解析结果与分支内容一致时什么也不做。
# - GITHUB_TOKEN 的推送与 PR 不触发其他工作流；由审阅者关闭再重新打开 PR，触发 PR 检查与锚点定义变化时的升级兼容矩阵。
# - 只提出变更，不合并：新单元默认登记为“通过”，与登记不一致的单元须由人判断。
# --dry-run 只生成提交与 PR 正文，不推送、不开 PR。
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
branch=automation/upgrade-matrix-anchors
base=main
dry_run=false
[[ "${1:-}" == --dry-run ]] && dry_run=true
files=(tests/upgrade-matrix/anchors.json tests/upgrade-matrix/expectations.json)

git fetch --quiet origin "$base"
open_pr=$(gh pr list --head "$branch" --base "$base" --state open --json number --jq '.[0].number // empty')
if [[ -n "$open_pr" ]] && git fetch --quiet origin "$branch" 2>/dev/null; then
  git switch --quiet -C "$branch" "origin/$branch"
else
  open_pr=
  git switch --quiet -C "$branch" "origin/$base"
fi

node scripts/testing/resolve-desktop-anchors.mjs --write
if git diff --quiet -- "${files[@]}"; then
  echo "upgrade matrix anchors already match published Desktop releases"
  exit 0
fi

body=$(mktemp)
trap 'rm -f "$body"' EXIT
base_anchors=$(git show "origin/$base:tests/upgrade-matrix/anchors.json")
BASE_ANCHORS="$base_anchors" node --input-type=module >"$body" <<'EOF'
import { readFileSync } from 'node:fs'
const read = path => JSON.parse(readFileSync(path, 'utf8'))
const before = JSON.parse(process.env.BASE_ANCHORS).anchors
const after = read('tests/upgrade-matrix/anchors.json').anchors
const features = read('tests/upgrade-matrix/host-features.json').anchors
const cells = read('tests/upgrade-matrix/expectations.json').cells
const knownTags = new Set(before.flatMap(anchor => anchor.desktop_releases.map(release => release.tag)))
const beforeIds = new Set(before.map(anchor => anchor.id))
const added = after.filter(anchor => !beforeIds.has(anchor.id))
const releases = after.flatMap(anchor =>
  anchor.desktop_releases.filter(release => !knownTags.has(release.tag)).map(release => ({ anchor, release })),
)
const unregistered = after.filter(anchor => !(anchor.id in features)).map(anchor => anchor.id)
const lines = [
  '## 目标',
  '',
  'Desktop 有新的公开发布，按[计划 051](docs/exec-plans/active/051-upgrade-compatibility-matrix.md)与',
  '[发布完整性](docs/security/release-integrity.md)追加升级兼容矩阵锚点。本 PR 由 `upgrade-matrix-anchors` 工作流生成。',
  '',
  '## 变更',
  '',
  ...releases.map(({ anchor, release }) =>
    `- Desktop \`${release.tag}\` → 锚点 \`${anchor.id}\`（Engine \`${anchor.engine_rev.slice(0, 12)}\`${beforeIds.has(anchor.id) ? '，已有锚点' : '，新锚点'}）`,
  ),
  `- 新增锚点 ${added.length} 个；矩阵单元共 ${cells.length} 个，新单元初始登记为 \`pass\`，既有登记保持不变。`,
  '',
  '## 合并前',
  '',
  unregistered.length
    ? `- [ ] 在 \`tests/upgrade-matrix/host-features.json\` 登记 ${unregistered.map(id => `\`${id}\``).join('、')} 的宿主能力（未登记时沿用最后一个已登记锚点的能力）。`
    : '- 所有锚点都已登记宿主能力。',
  '- [ ] 关闭再重新打开本 PR，触发 PR 检查与 `Engine real environment` 的升级兼容全矩阵（本 PR 由 `GITHUB_TOKEN` 创建或更新，不会自动触发；工作流之后追加提交时需再次重开）。',
  '- [ ] 读取全矩阵的 `matrix.md`；与登记不一致的单元按产品问题处理，不以修改登记代替判断。',
]
process.stdout.write(`${lines.join('\n')}\n`)
EOF

git -c user.name='github-actions[bot]' -c user.email='41898282+github-actions[bot]@users.noreply.github.com' \
  commit --quiet -m "test(upgrade-matrix): append anchors for new Desktop releases" -- "${files[@]}"

if $dry_run; then
  git --no-pager show --stat HEAD
  cat "$body"
  exit 0
fi

git push --quiet origin "$branch"
if [[ -n "$open_pr" ]]; then
  gh pr edit "$open_pr" --body-file "$body"
else
  gh pr create --base "$base" --head "$branch" \
    --title "test(upgrade-matrix): append anchors for new Desktop releases" --body-file "$body"
fi
