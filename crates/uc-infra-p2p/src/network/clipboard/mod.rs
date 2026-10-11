//! 普通剪贴板帧与持久接受回执；只消费 Uni 连接与流。
mod dispatch;
mod receiver;
pub use dispatch::ClipboardDispatchAdapter;
pub use receiver::ClipboardReceiverAdapter;
#[cfg(test)]
mod dispatch_tests;
#[cfg(test)]
mod receiver_tests;
#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod receiver_lifecycle_tests;
