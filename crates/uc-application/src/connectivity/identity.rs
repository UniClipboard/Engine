//! 连接资源键与业务授权分离；不同身份类型不能共享同一个连接池。
use std::hash::Hash;

/// 只用于资源登记与公平调度，不代表已取得成员资格。
pub trait ConnectionKey: Copy + Eq + Hash + Send + Sync + 'static {}

impl<T: Copy + Eq + Hash + Send + Sync + 'static> ConnectionKey for T {}
