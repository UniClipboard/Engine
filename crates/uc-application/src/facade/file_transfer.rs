//! 文件传输门面对外再导出(ADR-018 阶段 5)。

pub use crate::transfer::file::facade::{
    BeginReceiverTransfer, FileTransferFacade, FileTransferFacadeDeps, ReceiverTransferRegistration,
};
pub use crate::transfer::file::lifecycle::FileTransferLifecycleDeps;
pub use crate::transfer::file::session::FileTransferSession;
pub use crate::transfer::file::FileTransferApplicationError;
