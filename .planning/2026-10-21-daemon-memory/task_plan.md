# 降低压缩峰值与后台常驻

1. 对照当前核数并行、2 worker、单线程策略（进行中）。
2. 在既有 TransferCipherPort 与 V3/V4 往返测试上验证选定实现。
3. 将优化作为固定 Engine 提交供 Desktop 验证，运行共同的图片负载。

## 完整负责人
TransferCipherAdapter 继续拥有压缩与加密，调用方仍只调用 encrypt/decrypt。成功输出仍是可解码的既有协议，失败仍保留原始 source。无新增持久状态、重试或公开能力。

## Next Step
构建只依赖既有zstd的隔离诊断程序，比较时延、压缩率、峰值和静置PSS。
