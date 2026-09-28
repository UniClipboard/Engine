# 旧版资料快照

尚未使用 Engine 的 Desktop 版本无法由连接测试宿主现场写入资料，矩阵改为导入预先生成的快照，
再由当前源码打开同一资料目录。快照只含合成内容与测试口令，不含任何真实用户资料或凭据。

## desktop-0.19.4

- **来源程序**：GitHub Release `UniClipboard/UniClipboard` `v0.19.4` 的
  `uniclipboard-cli-0.19.4-x86_64-pc-windows-msvc.zip`，SHA-256
  `b0f9483a112468a70815f99719daee8a7ba5cf3cdc7b1d14fce2f764b783c8f2`（与该发布 `SHA256SUMS.txt` 一致）。
- **生成方式**：Windows 11 x64，`scripts/testing/create-legacy-desktop-profile.ps1 -Root <专属目录>`。
  使用便携模式与默认 profile，口令为连接测试宿主的固定口令，设备名 `LegacyFixture`，
  写入 3 条文本 `legacy fixture text 1..3`，两次 oneshot daemon 均正常关闭后打包。
- **内容**：`profile/` 是 0.19.4 的应用数据根目录原样内容（含 WAL，与用户正常退出后的状态一致）。
  剪贴板内容在库中为密文；`settings.json` 中只有合成设备名。
- **钥匙串**：便携模式把安全存储写成 `profile/keyring/<键名十六进制>.bin`。矩阵把这些文件解码为
  测试宿主的安全存储条目（当前只有 `kek:v1:profile:default`），与 Desktop 1.0 便携模式读取同一目录的行为一致。
- **完整性**：`SHA256SUMS` 覆盖 `profile/` 下每个文件；矩阵在使用前逐个校验，不一致即判为夹具无效。

快照更新时必须用同一脚本重新生成、整体替换 `profile/` 与 `SHA256SUMS`，并在此记录新的来源与日期。
