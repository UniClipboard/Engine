# 调研记录

- 当前口令同时保护本机 MasterKey，并用于 Space admission registration。
- 普通修改口令应保留 MasterKey 和历史密文，只重新保护 MasterKey 并替换 admission registration。
- 当前没有公开修改口令操作；`ensure_registration` 只补缺，不替换同 scope 的已有记录。
- V3 admission registration 绑定活动 Space control generation 和 keyslot generation。
- `re_pairing_required` 在旧关系清理后为 true，成功重新配对后清除。
- 一般多设备修改需要跨设备同步；本任务明确只允许当前成员集合只有本机的 Space。
- 现有定向测试已证明配对记录能用口令认证且绑定活动 V3 control generation。
- 原自动生成方案必须先展示、确认保存后启用，避免返回前中断导致用户拿不到新口令；用户自定义方案已取代该流程。
- `re_pairing_required` 表达设备关系尚未重建，修改口令成功后仍应保持，直到新设备实际完成配对。
- 口令替换期间需要与邀请签发共用互斥边界；确认前已有邀请必须先在真实发行方撤销。
- 跨 keyslot、系统安全存储和 SQLite 的替换需要受保护的前向恢复记录；普通内存补偿不足以覆盖进程中断。
- 2026-09-14 需求调整：新口令改由用户自定义，不再由 Engine 自动生成；产品负责输入与二次确认，Engine 只接收一次修改动作。
