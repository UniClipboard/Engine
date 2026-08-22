use std::sync::Arc;

use super::{
    ProfileGeneration, ProfileLifecycle, ProfileLifecycleRepositoryError,
    ProfileLifecycleRepositoryPort,
};

pub struct PrepareProfileLifecycleUseCase {
    repository: Arc<dyn ProfileLifecycleRepositoryPort>,
}

impl PrepareProfileLifecycleUseCase {
    pub fn new(repository: Arc<dyn ProfileLifecycleRepositoryPort>) -> Self {
        Self { repository }
    }

    pub fn execute(&self) -> Result<ProfileLifecycle, ProfileLifecycleRepositoryError> {
        if let Some(lifecycle) = self.repository.load()? {
            return Ok(lifecycle);
        }

        let lifecycle = ProfileLifecycle::new(ProfileGeneration::new());
        self.repository.compare_and_swap(None, &lifecycle)?;
        Ok(lifecycle)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct LifecycleRepository {
        lifecycle: Mutex<Option<ProfileLifecycle>>,
    }

    impl ProfileLifecycleRepositoryPort for LifecycleRepository {
        fn load(&self) -> Result<Option<ProfileLifecycle>, ProfileLifecycleRepositoryError> {
            Ok(self.lifecycle.lock().unwrap().clone())
        }

        fn compare_and_swap(
            &self,
            expected: Option<&ProfileLifecycle>,
            next: &ProfileLifecycle,
        ) -> Result<(), ProfileLifecycleRepositoryError> {
            let mut current = self.lifecycle.lock().unwrap();
            if current.as_ref() != expected {
                return Err(ProfileLifecycleRepositoryError::Conflict);
            }
            *current = Some(next.clone());
            Ok(())
        }
    }

    #[test]
    fn execute_creates_a_ready_lifecycle_only_when_missing() {
        let repository = Arc::new(LifecycleRepository::default());
        let prepare = PrepareProfileLifecycleUseCase::new(repository.clone());

        let created = prepare.execute().unwrap();
        let reopened = prepare.execute().unwrap();

        assert_eq!(created.state(), super::super::ProfileLifecycleState::Ready);
        assert_eq!(reopened, created);
    }
}
