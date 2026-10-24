# 网络栈 submodule 交付

## 完成标准

Engine 构建集成独占网络依赖来源；宿主只依赖 uc-engine，无根补丁即可编译完整修复链。
缺少或不符的 submodule 明确失败，由维护者更新 checkout 后重试。运行期业务、关闭、恢复责任不变。

## 切片

1. 在单一网络栈 fork 内固定 iroh、blobs、tickets、util 的路径依赖，保留许可证与补丁来源。
2. Engine 使用单一 submodule，移除根补丁；CI、日志来源审查和文档同步。
3. 独立 Git 消费者仅依赖 uc-engine，验证 submodule 获取、唯一网络依赖来源及编译。
4. 执行 DNS 重放、日志规则、workspace 编译、格式与架构门禁。

## 验收状态

实施中。Windows / 移动端运行验证尚未执行，不能沿用之前根补丁方案的同步成功作为本次验收。
