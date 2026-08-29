use openmls_rust_crypto::RustCrypto;
use openmls_traits::{crypto::OpenMlsCrypto, types::SignatureScheme};
use uc_core::membership::{
    ED25519_SIGNATURE_ALGORITHM_V1, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier,
};

pub struct OpenMlsHistoricalSignatureVerifier;

impl HistoricalMembershipSignatureVerifier for OpenMlsHistoricalSignatureVerifier {
    fn verify(
        &self,
        signature_algorithm_version: u16,
        public_key: &[u8],
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        if signature_algorithm_version != ED25519_SIGNATURE_ALGORITHM_V1 {
            return Err(HistoricalMembershipSignatureError::UnsupportedAlgorithm);
        }
        Ok(RustCrypto::default()
            .verify_signature(SignatureScheme::ED25519, payload, public_key, signature)
            .is_ok())
    }
}
