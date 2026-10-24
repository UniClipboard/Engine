//! 终态补发的纯规则：串行补发时刻、待确认容量与有界交换调度。
//!
//! 只表达可由调用方注入时间的判定，不含网络、任务或存储步骤；
//! 时间读数来自包含挂起的 `ElapsedClockPort`，读数不可用按过期处理。
mod exchange;
mod ledger;
mod retransmit;

#[cfg(test)]
mod tests;

pub use exchange::{
    ExchangeQueue, ExchangeRefusal, EXCHANGE_GLOBAL_ACTIVE, EXCHANGE_GLOBAL_QUEUED,
};
pub use ledger::{TerminalLedger, TERMINAL_GLOBAL_CAPACITY, TERMINAL_PEER_CAPACITY};
pub use retransmit::{
    RetransmitPoll, RetransmitStop, TerminalRetransmit, TERMINAL_RETENTION, TERMINAL_ROUND_LIMIT,
};
