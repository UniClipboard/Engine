mod encrypted_payload;
mod legacy_bootstrap;
mod revocation;
mod space_material;

#[cfg(test)]
mod tests;

use uc_core::membership::KeyEpochError;

use crate::security::InMemorySession;

pub struct DieselSpaceSecurityStore<E> {
    executor: E,
    session: InMemorySession,
}

impl<E> DieselSpaceSecurityStore<E> {
    pub fn new(executor: E, session: InMemorySession) -> Self {
        Self { executor, session }
    }
}

fn backend(error: impl std::fmt::Display) -> KeyEpochError {
    KeyEpochError::Repository(error.to_string())
}

fn epoch_to_i64(epoch: u64) -> Result<i64, KeyEpochError> {
    i64::try_from(epoch).map_err(|_| backend("group epoch exceeds SQLite range"))
}
