# Findings & Decisions

## Requirements
- A single unreadable historical payload must not make Engine startup unavailable.
- The upgrade must not silently discard ciphertext or persist decrypted user content.
- Affected history must remain identifiable as unavailable through the existing model.

## Research Findings
- Desktop profile database integrity checks pass; the startup failure occurs during V3 primary blob conversion.
- The affected profile contains 13 legacy UCBL v1 files: 11 fail AEAD opening, 2 open successfully. Six unreadable blobs are still referenced by `BlobReady` representations; five are orphan blob rows.
- The upgrade currently uses fail-fast `?` in `PrimaryPayloadConverter::convert_blobs`, so the first unreadable UCBL aborts the entire atomic generation build and daemon startup.
- Existing schema and domain behavior already define `payload_state = Lost`; resolvers and search availability treat it as unavailable.
- Existing Space-transition migration already preserves unreadable ciphertext and reports `preserved_unreadable_records`, establishing a repository precedent.
- `uc-infra` explicitly forbids recovering from encrypted-migration failures by deleting data and requires real-storage corruption/recovery tests.
- Profile storage upgrade is the existing deep module and sole owner of staging, conversion, verification, promotion, cleanup, and restart recovery; the behavior should remain behind `ensure_v3()` rather than add a caller-visible step interface.
- Runtime consumers already treat `Lost` as an unavailable payload. The remaining design question is where active V3 storage keeps the original unreadable UCBL bytes so source cleanup cannot destroy possible future recovery evidence.
- The two readable files are the newest timestamp cohort while earlier files fail under the same UCBL v1 parser and `blob_id`-bound AAD; referenced failures all belong to the local device. This strongly supports an earlier MasterKey generation no longer being available, rather than a parser/AAD regression.
- The target generation's blob-tree digest already authenticates every stored file. Preserved legacy ciphertext can remain at the same opaque blob basename in the active blob tree, while `blob.encryption_algo` distinguishes it from V3 and every referring representation is atomically marked `Lost`.

## Technical Decisions
| Decision | Rationale |
|----------|-----------|
| Treat the source ciphertext as immutable evidence | Future key/format recovery may make it readable, and deleting it violates migration policy. |
| Preserve unreadable UCBL inside the active V3 blob tree | Source cleanup remains safe, row identity stays intact, and the existing tree digest protects the copied bytes. |
| Abort on raw file I/O failure, degrade only after raw bytes are readable | Missing/unreadable media is a storage failure; an intact encrypted envelope that cannot be opened is payload unavailability. |

## Issues Encountered
| Issue | Resolution |
|-------|------------|
| Vite stale blank window obscured the daemon failure | Rebuilt Vite dependency metadata; React now mounts independently. |

## Resources
-

## 本轮实现结论

- 仅旧 UCBL 的具体 `AeadError::DecryptFailed` 被转换为保留原密文与 Lost；未知格式、解压失败、会话未就绪、密钥解析或 I/O 错误仍保留 source chain 并中止。此边界精确覆盖现场记录的 11 个 AEAD 失败，不泛化吞掉其他错误。
- EncryptedBlobStore 的读取和升级共用一个旧格式解码实现，升级先读取原字节；未增加正常 V3 reader 的兼容回退。
- Primary 校验除目录摘要外，对保留条目验证与来源字节相等、确为 AEAD 失败、全部引用 Lost 且具有固定原因；发布但 journal 尚未写入的恢复窗口同样执行。
- 复现测试改用真实 V1 framing 与错误旧 MasterKey，覆盖缺文件 source chain、未知格式中止、原子失败重试、Lost 状态篡改、保留文件篡改、重复读取及 V3 reader 拒绝旧密文。
- 完整运行期测试增加正常 V2 blob、两条引用的坏密文及孤立坏密文，通过生产 for_runtime/ensure_v3 验证来源清理后的保留与重复启动。

## 手动资料的新证据与修复范围

- GDB 在 derived_payloads 的 owner_security 断点确认真实错误来自文件清单 original_text 解码（原代码第 215 行）。
- 使用 Secret Service 只读取得当前 KEK，在内存中认证 keyslot 并按现有 HKDF/AAD 独立校验；不输出或持久化任何密钥或解密正文。当前 keyslot 正常，3/3 UCFS 路径无法认证，UCSR 搜索预览仅 5/55 可认证，活动剪贴板引用 1/1 可认证。尝试 keyslot 原 profile、default、dev 的历史派生盐均不能打开 UCFS。
- 新失败也是历史派生密文不可读，不是前端缓存或当前 keyslot 失效。
- 继续由 DerivedPayloadConverter 完整负责：遇到文件清单/搜索渲染的确定 AEAD 失败时，在候选 blob tree 内以固定名称保存完整 primary 数据库的 V3 加密恢复快照（全部行身份/原密文均包在密文内）；删除受影响 entry 的不可读文件清单投影并清空坏 render 字段，现有读取降级和搜索重建接手。
- 快照必须先耐久写入、回读认证并与 primary 原字节核对，再更新候选库；整体目录原子发布，blob tree 摘要覆盖恢复快照。恢复中缺失或损坏快照仍失败关闭。传输、发布、接收和活动引用不自动丢弃或降级。

## 修复后的真实验证

本地 Engine 重建 daemon 后，dev 资料首次启动和重启的 /health 均返回 HTTP 200、status=ok；正式流程已清理旧来源数据库，活动 generation 保留 1 份加密派生恢复快照。此前持续 loading 对应的 daemon 升级退出已解除；界面交互仍由用户手动验证。
