# 旧压缩载荷向量

`zstd-0.13.3-legacy-v3.zst`由冻结旧依赖链实际生成：zstd 0.13.3、zstd-safe 7.2.4、zstd-sys 2.0.16+zstd.1.5.7，启用原有zstdmt特性。
调用`zstd::bulk::compress(plaintext, 3)`；明文为`b"legacy compressed clipboard content\n".repeat(40_000)`，共1,440,000字节，仅为固定合成测试数据。
向量SHA-256为`cb6d987a90f22aac939fcc02a35b6c23205ef731ca62b7381de8e5c8e44706aa`。

[兼容测试](../compression_tests.rs)将这份不可变旧压缩数据装入既有UC3加密封装，再通过当前实际TransferCipherAdapter解密／解压，不能改为运行时用当前编码器重新生成“旧”载荷。
同源主机探针还对冻结旧链与0.14.3／8.2.0／2.1.2链验证空、文本、可压缩、字节循环及二进制五类数据双向精确互读，以及非空数据的解压容量不足拒绝；生产Cargo.lock没有引入旧依赖副本。
