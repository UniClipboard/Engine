use sha2::{Digest, Sha256};
use uc_core::membership::{AdmissionReplayId, LegacyUpgradeRequest};

pub(crate) fn request_transcript(request: &LegacyUpgradeRequest) -> Vec<u8> {
    let mut transcript = Vec::with_capacity(request.key_package().len() + 256);
    append_field(&mut transcript, b"uniclipboard-legacy-upgrade-request/v1");
    append_field(
        &mut transcript,
        request.source_device_id().as_str().as_bytes(),
    );
    append_field(
        &mut transcript,
        request.target_device_id().as_str().as_bytes(),
    );
    append_field(
        &mut transcript,
        request.descriptor().upgrade_id().as_bytes(),
    );
    append_field(
        &mut transcript,
        request
            .descriptor()
            .protection_group_id()
            .map_or(&[][..], |id| id.as_str().as_bytes()),
    );
    append_field(
        &mut transcript,
        match request.kind() {
            uc_core::membership::LegacyUpgradeRequestKind::Admission => b"admission",
            uc_core::membership::LegacyUpgradeRequestKind::ReadmissionProbe => b"readmission-probe",
            uc_core::membership::LegacyUpgradeRequestKind::ReadmissionConfirmation => {
                b"readmission-confirmation"
            }
        },
    );
    append_field(&mut transcript, request.key_package());
    transcript
}

pub(crate) fn request_id(request: &LegacyUpgradeRequest) -> AdmissionReplayId {
    let mut hasher = Sha256::new();
    hasher.update(request_transcript(request));
    hasher.update((request.proof().len() as u64).to_be_bytes());
    hasher.update(request.proof());
    AdmissionReplayId::from_bytes(hasher.finalize().into())
}

fn append_field(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as u64).to_be_bytes());
    target.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use uc_core::ids::DeviceId;
    use uc_core::membership::{
        LegacyUpgradeDescriptor, LegacyUpgradeId, LegacyUpgradeRequest, ProtectionGroupId,
    };

    use super::{request_id, request_transcript};

    fn request(target: &str, group: &str, key_package: Vec<u8>) -> LegacyUpgradeRequest {
        LegacyUpgradeRequest::unsigned(
            DeviceId::new("device-a"),
            DeviceId::new(target),
            LegacyUpgradeDescriptor::ready(
                LegacyUpgradeId::from_bytes([1; 32]),
                ProtectionGroupId::from_string(group).unwrap(),
            ),
            key_package,
        )
        .with_proof(vec![7, 8, 9])
    }

    #[test]
    fn transcript_binds_sender_recipient_group_and_key_package() {
        let base = request("device-b", "group-a", vec![1, 2, 3]);
        let different_target = request("device-c", "group-a", vec![1, 2, 3]);
        let different_group = request("device-b", "group-b", vec![1, 2, 3]);
        let different_package = request("device-b", "group-a", vec![4, 5, 6]);

        assert_ne!(
            request_transcript(&base),
            request_transcript(&different_target)
        );
        assert_ne!(
            request_transcript(&base),
            request_transcript(&different_group)
        );
        assert_ne!(
            request_transcript(&base),
            request_transcript(&different_package)
        );
    }

    #[test]
    fn replay_id_binds_the_request_proof() {
        let base = request("device-b", "group-a", vec![1, 2, 3]);
        let changed_proof = LegacyUpgradeRequest::unsigned(
            *base.source_device_id(),
            *base.target_device_id(),
            base.descriptor().clone(),
            base.key_package().to_vec(),
        )
        .with_proof(vec![9, 8, 7]);

        assert_ne!(request_id(&base), request_id(&changed_proof));
    }
}
