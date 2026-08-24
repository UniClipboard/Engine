//! 在当前 Space 中正式发起一次成员移除。
//!
//! 本机先验证发起资格和目标成员，生成并签名 `RemoveDevice`，再通过版本比较
//! 保存成员历史。保存成功只表示本机已经采用该移除；其他成员仍需各自接受或
//! 拒绝。网络传播由成员后台运行期负责，可在设备离线或进程重启后继续。
mod deps;
mod error;
mod model;
mod use_case;

#[cfg(test)]
mod tests;

pub(crate) use deps::InitiateSpaceMemberRemovalDeps;
pub use error::InitiateSpaceMemberRemovalError;
pub use model::InitiateSpaceMemberRemovalResult;
pub(crate) use use_case::InitiateSpaceMemberRemovalUseCase;
