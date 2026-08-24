mod facade;

pub use crate::space::admission::cancel_space_join::CancelSpaceJoinError;
pub use crate::space::admission::query_space_join_status::QuerySpaceJoinStatusError;
pub use crate::space::admission::recover_space_join_completion::{
    PendingJoinerCompleteAck, RecoverSpaceJoinCompletionError,
};
pub use facade::SpaceJoinFacade;
