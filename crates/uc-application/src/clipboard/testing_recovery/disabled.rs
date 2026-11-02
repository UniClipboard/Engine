//! 禁用实现没有观测注册表或控制能力。
use uc_core::clipboard::EntryDeliveryRecord;

pub(crate) fn record_fenced_result(_: &EntryDeliveryRecord) {}
