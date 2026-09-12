mod encrypted_payload;
mod legacy_bootstrap;
mod revocation;
mod space_material;

#[cfg(test)]
mod tests;

use uc_core::membership::KeyEpochError;

use crate::space::InMemorySession;

pub struct DieselSpaceSecurityStore<E> {
    executor: E,
    session: InMemorySession,
}

impl<E> DieselSpaceSecurityStore<E> {
    pub fn new(executor: E, session: InMemorySession) -> Self {
        Self { executor, session }
    }
}

fn backend(error: impl Into<anyhow::Error>) -> KeyEpochError {
    KeyEpochError::Repository(error.into())
}

fn epoch_to_i64(epoch: u64) -> Result<i64, KeyEpochError> {
    i64::try_from(epoch).map_err(backend)
}

#[cfg(test)]
mod failure_contract_tests {
    use super::*;
    #[test]
    fn backend_failure_retains_its_source_without_public_private_text() {
        let error = backend(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "PRIVATE_STORAGE_PATH",
        ));
        let source = std::error::Error::source(&error).expect("真实存储错误不能被字符串化");
        let source = source
            .downcast_ref::<std::io::Error>()
            .expect("原始 I/O 来源");
        assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(!error.to_string().contains("PRIVATE_STORAGE_PATH"));
        assert!(!format!("{error:?}").contains("PRIVATE_STORAGE_PATH"));
    }
}
