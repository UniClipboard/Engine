//! 终态补发的纯规则：串行补发时刻、待确认容量与有界交换调度。
//!
//! 只表达可由调用方注入时间的判定，不含网络、任务或存储步骤；
//! 时间读数由调用方注入，须包含系统挂起；不可用读数按过期处理。
mod budget;
mod exchange;
mod ledger;
mod retransmit;

#[cfg(test)]
mod tests;

pub use budget::{BudgetStatus, ElapsedBudget};
pub use exchange::{
    ExchangePriority, ExchangeQueue, ExchangeRefusal, EXCHANGE_GLOBAL_ACTIVE,
    EXCHANGE_GLOBAL_QUEUED, EXCHANGE_PEER_QUEUED,
};
pub use ledger::{TerminalLedger, TERMINAL_GLOBAL_CAPACITY, TERMINAL_PEER_CAPACITY};
pub use retransmit::{
    RetransmitEffect, RetransmitInput, RetransmitStop, TerminalRetransmit, TERMINAL_RETENTION,
    TERMINAL_ROUND_LIMIT,
};
