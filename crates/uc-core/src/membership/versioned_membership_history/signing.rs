//! 签名材料的规范字段排列，所有记录共用同一实现。

use super::{MembershipAdmissionV2, MembershipEventId, MembershipOperationV2};

pub(super) fn append_operation(bytes: &mut Vec<u8>, operation: &MembershipOperationV2) {
    match operation {
        MembershipOperationV2::AddDevice { admission } => {
            bytes.push(1);
            append_admission(bytes, admission);
        }
        MembershipOperationV2::RemoveDevice { member } => {
            bytes.push(2);
            bytes.extend_from_slice(member.as_bytes());
        }
    }
}

pub(super) fn append_admission(bytes: &mut Vec<u8>, admission: &MembershipAdmissionV2) {
    bytes.extend_from_slice(admission.facts.member_instance.as_bytes());
    append_field(bytes, admission.facts.device_id.as_str().as_bytes());
    append_field(bytes, admission.facts.device_name.as_bytes());
    append_field(
        bytes,
        admission.facts.identity_fingerprint.as_display().as_bytes(),
    );
    append_field(bytes, &admission.facts.transport_public_key);
    append_field(bytes, &admission.facts.transport_address_blob);
    append_field(bytes, &admission.facts.identity_signature);
    bytes.extend_from_slice(
        &admission
            .membership_credential
            .credential_format_version
            .to_be_bytes(),
    );
    bytes.extend_from_slice(
        &admission
            .membership_credential
            .signature_algorithm_version
            .to_be_bytes(),
    );
    append_field(bytes, &admission.membership_credential.public_key);
    bytes.extend_from_slice(admission.membership_credential.credential_id.as_bytes());
    bytes.extend_from_slice(&admission.resume_public_key_digest);
    bytes.extend_from_slice(&admission.security_commitment_id);
}

pub(super) fn append_field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

pub(super) fn append_optional_event_id(bytes: &mut Vec<u8>, event_id: Option<MembershipEventId>) {
    match event_id {
        Some(event_id) => {
            bytes.push(1);
            bytes.extend_from_slice(event_id.as_bytes());
        }
        None => bytes.push(0),
    }
}

pub(super) fn append_optional_digest(bytes: &mut Vec<u8>, digest: Option<[u8; 32]>) {
    match digest {
        Some(digest) => {
            bytes.push(1);
            bytes.extend_from_slice(&digest);
        }
        None => bytes.push(0),
    }
}
