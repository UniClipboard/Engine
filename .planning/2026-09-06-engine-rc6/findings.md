# 发布发现

- 当前 Engine 与最新公开预发布均为 1.1.0-rc.5；rc.6 标签不存在。
- main 比远端领先 12 个已提交改动，无远端新增提交，工作区开始时干净。
- 发布工作流支持 workflow_dispatch，必须明确 dry_run=false、prerelease=true，避免 tag push 路径没有预发布标志。
- 发布说明仍提到联通门禁，实际工作流和测试已明确移除；历史提交 0ff09048 为 fix(release): notify consumers without interop gate。
- 业务 upgrade_space 中的 rc.5 是最低版本规则，不是发布版本，不随本次发版机械替换。
