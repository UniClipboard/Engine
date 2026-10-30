# apt 下载缓存

本 action 只供 `pr-check.yml` 的 Ubuntu job 安装系统工具。调用方保留原包名；每轮先执行
`apt-get update`，再由 apt 解析当前依赖。安装失败直接使步骤失败，下次运行重新解析与安装。
缓存只含 runner 临时目录中的 `.deb`，不含 apt 索引、dpkg 数据库或已安装文件。

key 包含实现版本、runner OS/架构、发行版、dpkg 架构、包集合、apt 配置、源配置，以及本轮
解析出的下载 URI、大小和 SHA-256。没有宽泛回退 key；源或版本变化会重新下载。缓存未命中、
损坏、过期或被驱逐均走正常 apt 下载。恢复后逐包对照本轮解析校验，不信任缓存内的清单，
也不跳过安装或降低 apt 的签名／哈希要求。缓存故障不能作为软件源的替代品。

沿用 GitHub Actions 的 ref 隔离：PR merge ref 写入的缓存不能被主线或其他 PR 恢复。
不得将此 action 改接 `pull_request_target` 或共享的特权缓存。详见
[GitHub 缓存作用域](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching#restrictions-for-accessing-a-cache)。

本地聚焦回归：`python3 scripts/ci/apt-download-cache.test.py`。实际安装和托管缓存传输仍需
Ubuntu CI 验证；缓存命中不代表系统工具已经安装，也不代表已证明端到端提速。
