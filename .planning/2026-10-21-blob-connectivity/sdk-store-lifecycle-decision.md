# 固定SDK store关闭的设计取舍

2026-10-23 用户已明确批准“允许修订同一 fork，保持协议与格式”。据此推进兼容的 store 生命周期修复；不再等待该范围的确认。旧修订的协议／磁盘证据保留，新修订必须重新验证。

当前规格固定iroh-blobs 0.103.0、修订`124780fb69768b8506565b2760c435182f9e2d20`。真实受管资源验收暴露的缺口属于store后台寿命，不能通过放宽线程断言或关闭GC交付。

| 实际场景 | 30秒后线程／初值 | iroh类线程 | 当前runtime任务（含采样器） |
| --- | --- | --- | --- |
| 正常GC，真实2×1GiB | 45／5 | 未采类别 | 1 |
| 正常GC，仅双Engine配对关闭 | 45／5 | 30 | 1 |
| 临时关闭GC，仅双Engine配对关闭 | 18／5 | 3 | 1 |

所有场景都真实运行，原始计数分别在`managed-idle-evidence/`、`managed-idle-setup-evidence/`、`managed-idle-no-gc-evidence/`。禁用GC实验已撤销；不把该实验视为合格方案。30秒后主runtime任务已回初值加一个采样器，但SDK私有runtime不在该任务计数中。

固定SDK源码事实：

- `src/store/fs.rs::FsStore::load_with_opts`为每个store创建独立runtime，并在有配置时启动GC；没有向调用者返回该任务的join句柄。
- `src/store/gc.rs::run_gc`在每轮前睡眠配置间隔并持有store；本仓间隔为一小时。关闭数据库不会主动唤醒这次睡眠。
- `src/store/fs.rs::Actor::handle_command(Shutdown)`只转交metadata actor；外层actor仍等待命令。`api::Store::shutdown`确认这次请求，不代表调用者已join所有store后台工作。
- 关闭GC后仍有3个iroh类线程，尚未取得它们的完整堆栈／退出责任证明。`RtWrapper::drop`在自有runtime上调用`block_in_place(drop(runtime))`是待验证的候选，不能写成已证实根因。

推荐取舍：允许修订同一SDK fork的本地store生命周期，保持SDK版本系列、ALPN、ticket、内容校验及磁盘格式不变。SDK需提供可等待的完整关闭：停止GC排队、等待已开始GC和导入导出、关闭metadata与actor、在正确runtime归属上完成私有线程池退出。随后本仓原BlobStoreLifecyclePort消费该完整能力，不新增Engine步骤或查询接口。修订变化后重跑阶段0的SDK停止证明、协议向量、实际旧新双角色与资源矩阵；不能直接沿用旧修订结论。

完整负责人为 SDK 文件 store 的生命周期监督器；调用方仍只调用既有 `Store::shutdown`。成功确认必须晚于 GC 停止、已接收任务结算、metadata 落盘、actor 退出和私有 runtime 回收；下层异常应使关闭失败，不能提前发送成功确认。加载失败及全部引用自然释放也由同一监督器回收。Engine 不编排这些内部步骤。重启仍读取原磁盘格式，业务重试继续归 Application transfer。

修复前严格回归保持明确跳过并标注已知缺口，原2×1GiB流式门禁和实际资源计数保留。修复后取消跳过并重跑最小 store 回归、完整 Engine 关闭和真实2×1GiB用例，再执行 SDK 停止及兼容矩阵；不能把现有流级停止或字节通过替代完整后台退出。


修复中证据：独立Store实际红测为2.024秒、关闭确认后10个SDK线程；新监督器的公开API回归通过0.220秒，包含GC开关、原导入屏障、读回、自然释放和加载失败。新SDK真实Engine关闭后SDK命名线程已退出，残余10个线程的gdb栈确认为进程级Rayon池，名称继承Tokio创建线程。资源测试保留冷计数，并在只初始化该共享CPU池后取正式基线；既有严格退出条件不放宽。校准后的Engine／2×1GiB、新git修订及完整兼容矩阵仍待验证。


最终候选固定同一fork修订152c42a9217f1377a998b271e0052311e8ada789。监督线程自身在私有runtime回收后送达确认并退出，不将确认任务委托给加载方runtime；SDK公开API跨runtime红测0.376秒失败后修复，最新公开API回归0.374秒及原文件／GC20项2.997秒通过。失败回应同样延迟至私有runtime退出。正式Engine／大文件及本仓门禁以该最终修订的重新执行结果为准。
