use async_trait::async_trait;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use tokio::sync::Mutex;

use uc_core::ids::{SessionId, SpaceId};
use uc_core::ports::space::ProofPort;
use uc_core::space_access::{ProofDerivedKey, SpaceAccessProofArtifact};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProofCacheKey {
    pairing_session_id: String,
    space_id: String,
    challenge_nonce: [u8; 32],
}

pub struct HmacProofAdapter {
    key_cache: Mutex<HashMap<ProofCacheKey, [u8; 32]>>,
}

impl Default for HmacProofAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl HmacProofAdapter {
    pub fn new() -> Self {
        Self {
            key_cache: Mutex::new(HashMap::new()),
        }
    }

    fn payload(
        pairing_session_id: &SessionId,
        space_id: &SpaceId,
        challenge_nonce: [u8; 32],
    ) -> Vec<u8> {
        let session = pairing_session_id.as_str().as_bytes();
        let space = space_id.as_ref().as_bytes();

        let mut payload =
            Vec::with_capacity(8 + session.len() + space.len() + challenge_nonce.len());
        payload.extend_from_slice(&(session.len() as u32).to_be_bytes());
        payload.extend_from_slice(session);
        payload.extend_from_slice(&(space.len() as u32).to_be_bytes());
        payload.extend_from_slice(space);
        payload.extend_from_slice(&challenge_nonce);
        payload
    }

    fn cache_key(
        pairing_session_id: &SessionId,
        space_id: &SpaceId,
        challenge_nonce: [u8; 32],
    ) -> ProofCacheKey {
        ProofCacheKey {
            pairing_session_id: pairing_session_id.as_str().to_string(),
            space_id: space_id.as_ref().to_string(),
            challenge_nonce,
        }
    }

    fn compute_hmac(
        pairing_session_id: &SessionId,
        space_id: &SpaceId,
        challenge_nonce: [u8; 32],
        master_key_bytes: &[u8],
    ) -> anyhow::Result<Vec<u8>> {
        let payload = Self::payload(pairing_session_id, space_id, challenge_nonce);
        let mut mac = HmacSha256::new_from_slice(master_key_bytes)?;
        mac.update(&payload);
        Ok(mac.finalize().into_bytes().to_vec())
    }

    fn verify_hmac(
        pairing_session_id: &SessionId,
        space_id: &SpaceId,
        challenge_nonce: [u8; 32],
        key_bytes: &[u8],
        tag: &[u8],
    ) -> anyhow::Result<bool> {
        let payload = Self::payload(pairing_session_id, space_id, challenge_nonce);
        let mut mac = HmacSha256::new_from_slice(key_bytes)?;
        mac.update(&payload);
        Ok(mac.verify_slice(tag).is_ok())
    }
}

#[async_trait]
impl ProofPort for HmacProofAdapter {
    async fn build_proof(
        &self,
        pairing_session_id: &SessionId,
        space_id: &SpaceId,
        challenge_nonce: [u8; 32],
        derived_key: &ProofDerivedKey,
    ) -> anyhow::Result<SpaceAccessProofArtifact> {
        let key_bytes = derived_key.as_bytes();
        tracing::debug!(
            session_id = %pairing_session_id,
            space_id = %space_id,
            "building HMAC proof"
        );

        let proof_bytes =
            Self::compute_hmac(pairing_session_id, space_id, challenge_nonce, key_bytes)?;

        let cache_key = Self::cache_key(pairing_session_id, space_id, challenge_nonce);
        let mut cached = [0u8; 32];
        cached.copy_from_slice(key_bytes);
        self.key_cache.lock().await.insert(cache_key, cached);

        Ok(SpaceAccessProofArtifact {
            pairing_session_id: pairing_session_id.clone(),
            space_id: space_id.clone(),
            challenge_nonce,
            proof_bytes,
        })
    }

    async fn verify_proof(
        &self,
        proof: &SpaceAccessProofArtifact,
        expected_nonce: [u8; 32],
    ) -> anyhow::Result<bool> {
        if proof.challenge_nonce != expected_nonce {
            tracing::warn!(
                session_id = %proof.pairing_session_id,
                space_id = %proof.space_id,
                "proof verification failed: challenge nonce mismatch"
            );
            return Ok(false);
        }

        let cache_key = Self::cache_key(
            &proof.pairing_session_id,
            &proof.space_id,
            proof.challenge_nonce,
        );
        let master_key = {
            let cache = self.key_cache.lock().await;
            cache.get(&cache_key).copied()
        };

        let Some(master_key) = master_key else {
            tracing::warn!(
                session_id = %proof.pairing_session_id,
                "proof verification failed: no transcript credential cached"
            );
            return Ok(false);
        };

        let matched = Self::verify_hmac(
            &proof.pairing_session_id,
            &proof.space_id,
            proof.challenge_nonce,
            &master_key,
            &proof.proof_bytes,
        )?;
        if !matched {
            tracing::warn!(
                session_id = %proof.pairing_session_id,
                space_id = %proof.space_id,
                proof_len = proof.proof_bytes.len(),
                "proof verification failed: HMAC mismatch"
            );
        } else {
            tracing::info!(
                session_id = %proof.pairing_session_id,
                "proof verification succeeded"
            );
        }

        Ok(matched)
    }

    async fn verify_proof_with_key(
        &self,
        proof: &SpaceAccessProofArtifact,
        expected_nonce: [u8; 32],
        verification_key: &ProofDerivedKey,
    ) -> anyhow::Result<bool> {
        if proof.challenge_nonce != expected_nonce {
            return Ok(false);
        }
        Self::verify_hmac(
            &proof.pairing_session_id,
            &proof.space_id,
            proof.challenge_nonce,
            verification_key.as_bytes(),
            &proof.proof_bytes,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;
    use std::sync::{Arc, Mutex as StdMutex};
    use tracing::Level;

    #[derive(Clone, Default)]
    struct CapturedWriter(Arc<StdMutex<Vec<u8>>>);

    impl CapturedWriter {
        fn dump(&self) -> String {
            String::from_utf8(self.0.lock().expect("lock captured logs").clone())
                .expect("captured logs should be UTF-8")
        }
    }

    impl Write for CapturedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("lock captured logs")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedWriter {
        type Writer = CapturedWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn proof_logs_do_not_include_key_material() -> anyhow::Result<()> {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .with_max_level(Level::DEBUG)
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        let adapter = HmacProofAdapter::new();
        let session_id = SessionId::new("session-log-redaction".to_string());
        let space_id = SpaceId::from_str("space-log-redaction");
        let nonce = [0x42; 32];
        let mut key = [0x55; 32];
        key[..4].copy_from_slice(&[0xab, 0xcd, 0xef, 0x01]);
        let derived_key = ProofDerivedKey::from_bytes(key);

        let mut proof = adapter
            .build_proof(&session_id, &space_id, nonce, &derived_key)
            .await?;
        proof.proof_bytes[0] ^= 0xff;
        assert!(!adapter.verify_proof(&proof, nonce).await?);

        let logs = writer.dump();
        let full_key_hex = key
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let key_debug = format!("{key:?}");
        assert!(
            !logs.contains("abcdef01"),
            "proof key material leaked into logs: {logs}"
        );
        assert!(
            !logs.contains(&full_key_hex),
            "complete key leaked into logs"
        );
        assert!(!logs.contains(&key_debug), "raw key bytes leaked into logs");
        Ok(())
    }
}
