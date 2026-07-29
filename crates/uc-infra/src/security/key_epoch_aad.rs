use uc_core::ids::SpaceId;
use uc_core::membership::{ContentKeyId, ContentKeyPurpose, GroupEpoch};

pub(crate) fn bind(
    format: &[u8],
    space_id: &SpaceId,
    epoch: GroupEpoch,
    content_key_id: &ContentKeyId,
    purpose: ContentKeyPurpose,
    business_aad: &[u8],
) -> Vec<u8> {
    let mut aad = Vec::new();
    append_field(&mut aad, b"uniclipboard-key-epoch-aad-v1");
    append_field(&mut aad, format);
    append_field(&mut aad, space_id.as_ref().as_bytes());
    aad.extend_from_slice(&epoch.value().to_le_bytes());
    append_field(&mut aad, content_key_id.as_str().as_bytes());
    append_field(&mut aad, purpose.as_str().as_bytes());
    append_field(&mut aad, business_aad);
    aad
}

fn append_field(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_le_bytes());
    output.extend_from_slice(value);
}
