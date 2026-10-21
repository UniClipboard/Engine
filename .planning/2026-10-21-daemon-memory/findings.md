Desktop当前固定Engine f0627a8b；独立工作树从该提交创建，避免包含其他任务。
既有实测显示压缩类同时活跃120.15MiB；超过1MiB即启用available_parallelism个worker。

已固定zstd-sys2.0.16做3进程×10次对照。单线程3MiB PNG0.94ms（原1.89ms），24MiB PNG7.08ms（原8.47ms），整进程峰值RSS下降约50MiB；32MiB重复文本11.0ms（原7.96ms）。选择bulk单线程，移除zstdmt依赖特性；协议/加密格式不变。
