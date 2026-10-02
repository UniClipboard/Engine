# 发现

- Ready 模式 `FactoryResetSpace` 走 `ProfileFactoryResetFacade`，不经过 `session_supervisor::reset_space` / `install_new_session`
  （那条路径属于 `Operation::ResetSpace`）。t-0142 的“install_new_session 失败”假设被源码排除。
- `ProfileRuntimeStopper` 停止会话与进程任务、关闭安全会话、`clear_factory()`；随后 `current_facade()` 返回 1103。
- 恢复模式的同一操作会发布 `restart_required=true`；Ready 模式什么也不发布，直接报告成功。
- 移动端宿主没有剪贴板变化流；Desktop 的 `take_change_stream` 只能取一次，第二次返回 `Ok(None)`，
  因此重建运行期必须复用同一条变化流，否则静默丢失监听。
