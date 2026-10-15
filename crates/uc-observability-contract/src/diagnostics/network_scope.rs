//! 第三方网络库调用的独立观测作用域。

use std::future::Future;

use tracing::instrument::WithSubscriber;
use tracing::{Dispatch, Instrument, Span};

/// 网络作用域 span 的 target。只有独立 network 日志层接受它；合同、模块与宿主输出都忽略。
pub const NETWORK_SCOPE_TARGET: &str = "uc.network";

/// 在无业务 parent 的网络作用域里驱动第三方网络库的建链 Future。
///
/// 库内部会 spawn 长寿命的传输 driver，driver 捕获 poll 时的当前 span。作用域 span 显式没有 parent，
/// driver 因此捕获它而不是调用方的业务 span：不延长业务 span 的生命周期，业务调用树里也没有网络库的节点。
///
/// 当前 subscriber 没有任何层接受网络作用域（进程没有安装独立 network 日志）时，span 是禁用的，
/// 进入它不会清除当前的业务 span；此时退回到完全静默的 dispatch，保持业务 span 的隔离不变。
pub async fn within_network_scope<F: Future>(future: F) -> F::Output {
    let scope = network_scope();
    if scope.is_disabled() {
        future
            .with_subscriber(Dispatch::new(tracing::subscriber::NoSubscriber::default()))
            .await
    } else {
        future.instrument(scope).await
    }
}

fn network_scope() -> Span {
    tracing::info_span!(target: NETWORK_SCOPE_TARGET, parent: None, "network_scope")
}
