use super::super::message::{
    AdmissionRole, SpaceAdmissionBodyV1, SpaceAdmissionMessageKind, SpaceAdmissionRejectionReason,
};
use super::*;

mod construct;
mod helper;
mod joiner;
mod sponsor;
mod terminal;

fn message_matches_evidence(
    message: &SpaceAdmissionEnvelopeV1,
    evidence: &AdmissionMessageEvidence,
) -> bool {
    message.header().sender_role() == evidence.sender_role()
        && message.header().sender_sequence() == evidence.sender_sequence()
        && message.header().message_id() == evidence.message_id()
        && message.header().predecessor_message_id() == evidence.predecessor_message_id()
}
