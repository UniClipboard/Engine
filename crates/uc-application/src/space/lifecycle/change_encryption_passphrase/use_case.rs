use std::sync::Arc;

use uc_core::crypto::domain::Passphrase;

use super::{
    ApplyEncryptionPassphraseChangePort, ApplyEncryptionPassphraseChangePortError,
    ChangeEncryptionPassphraseError, RetirePairingInvitationsPort,
};
use crate::space::membership::{CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort};

pub(crate) struct ChangeEncryptionPassphraseUseCase {
    member_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    invitations: Arc<dyn RetirePairingInvitationsPort>,
    apply: Arc<dyn ApplyEncryptionPassphraseChangePort>,
}

impl ChangeEncryptionPassphraseUseCase {
    pub(crate) fn new(
        member_scope: Arc<dyn CurrentSpaceMemberScopePort>,
        invitations: Arc<dyn RetirePairingInvitationsPort>,
        apply: Arc<dyn ApplyEncryptionPassphraseChangePort>,
    ) -> Self {
        Self {
            member_scope,
            invitations,
            apply,
        }
    }

    pub(crate) async fn execute(
        &self,
        passphrase: &Passphrase,
        passphrase_confirmation: &Passphrase,
    ) -> Result<(), ChangeEncryptionPassphraseError> {
        if passphrase != passphrase_confirmation {
            return Err(ChangeEncryptionPassphraseError::PassphraseMismatch);
        }
        self.ensure_eligible().await?;
        self.invitations
            .retire_all()
            .await
            .map_err(ChangeEncryptionPassphraseError::invitation)?;
        self.apply
            .apply_encryption_passphrase_change(passphrase)
            .await
            .map_err(|error| match error {
                ApplyEncryptionPassphraseChangePortError::RecoveryRequired { source } => {
                    ChangeEncryptionPassphraseError::recovery(source)
                }
                ApplyEncryptionPassphraseChangePortError::Unavailable { source } => {
                    ChangeEncryptionPassphraseError::unavailable(source)
                }
            })?;
        Ok(())
    }

    pub(crate) async fn ensure_ready(&self) -> Result<(), ChangeEncryptionPassphraseError> {
        self.apply
            .ensure_encryption_passphrase_change_ready()
            .await
            .map_err(|error| match error {
                ApplyEncryptionPassphraseChangePortError::RecoveryRequired { source } => {
                    ChangeEncryptionPassphraseError::recovery(source)
                }
                ApplyEncryptionPassphraseChangePortError::Unavailable { source } => {
                    ChangeEncryptionPassphraseError::unavailable(source)
                }
            })
    }

    async fn ensure_eligible(&self) -> Result<(), ChangeEncryptionPassphraseError> {
        let scope = self
            .member_scope
            .snapshot()
            .await
            .map_err(|error| match error {
                CurrentSpaceMemberScopeError::Locked
                | CurrentSpaceMemberScopeError::NoCurrentSpace => {
                    ChangeEncryptionPassphraseError::Locked
                }
                CurrentSpaceMemberScopeError::RecoveryRequired => {
                    ChangeEncryptionPassphraseError::MembershipRecoveryRequired
                }
                CurrentSpaceMemberScopeError::Unavailable => {
                    ChangeEncryptionPassphraseError::MembershipUnavailable
                }
            })?;
        if !scope.local_member_active {
            return Err(ChangeEncryptionPassphraseError::MembershipRecoveryRequired);
        }
        if !scope.usable_peer_device_ids.is_empty() || !scope.paused_peer_devices.is_empty() {
            return Err(ChangeEncryptionPassphraseError::MultipleDevices);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use uc_core::ids::DeviceId;

    use super::*;
    use crate::space::membership::{
        CurrentSpaceMemberScope, PausedSpaceMember, SpaceMemberPauseReason,
    };

    struct MemberScope(Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError>);

    #[async_trait]
    impl CurrentSpaceMemberScopePort for MemberScope {
        async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
            self.0.clone()
        }
    }

    #[derive(Default)]
    struct Invitations(Mutex<usize>);

    #[async_trait]
    impl RetirePairingInvitationsPort for Invitations {
        async fn retire_all(&self) -> Result<(), anyhow::Error> {
            *self.0.lock().unwrap() += 1;
            Ok(())
        }
    }

    #[derive(Default)]
    struct Apply(Mutex<Vec<String>>);

    #[async_trait]
    impl ApplyEncryptionPassphraseChangePort for Apply {
        async fn ensure_encryption_passphrase_change_ready(
            &self,
        ) -> Result<(), ApplyEncryptionPassphraseChangePortError> {
            Ok(())
        }

        async fn apply_encryption_passphrase_change(
            &self,
            passphrase: &Passphrase,
        ) -> Result<(), ApplyEncryptionPassphraseChangePortError> {
            self.0.lock().unwrap().push(passphrase.expose().to_owned());
            Ok(())
        }
    }

    fn local_only() -> CurrentSpaceMemberScope {
        CurrentSpaceMemberScope {
            revision: 1,
            local_member_active: true,
            usable_peer_device_ids: Vec::new(),
            paused_peer_devices: Vec::new(),
        }
    }

    fn use_case(
        scope: Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError>,
    ) -> (
        ChangeEncryptionPassphraseUseCase,
        Arc<Invitations>,
        Arc<Apply>,
    ) {
        let invitations = Arc::new(Invitations::default());
        let apply = Arc::new(Apply::default());
        (
            ChangeEncryptionPassphraseUseCase::new(
                Arc::new(MemberScope(scope)),
                invitations.clone(),
                apply.clone(),
            ),
            invitations,
            apply,
        )
    }

    #[tokio::test]
    async fn change_retires_invitations_before_applying_user_passphrase() {
        let (use_case, invitations, apply) = use_case(Ok(local_only()));
        let passphrase = Passphrase::new("user-selected-passphrase");

        use_case.execute(&passphrase, &passphrase).await.unwrap();

        assert_eq!(*invitations.0.lock().unwrap(), 1);
        assert_eq!(*apply.0.lock().unwrap(), [passphrase.expose()]);
    }

    #[tokio::test]
    async fn mismatched_confirmation_is_rejected_before_changing_anything() {
        let (use_case, invitations, apply) = use_case(Ok(local_only()));

        let error = use_case
            .execute(
                &Passphrase::new("user-selected-passphrase"),
                &Passphrase::new("different-passphrase"),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ChangeEncryptionPassphraseError::PassphraseMismatch
        ));
        assert_eq!(*invitations.0.lock().unwrap(), 0);
        assert!(apply.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn ordinary_single_device_space_can_change_passphrase() {
        let (use_case, invitations, apply) = use_case(Ok(local_only()));
        let passphrase = Passphrase::new("user-selected-passphrase");

        use_case.execute(&passphrase, &passphrase).await.unwrap();

        assert_eq!(*invitations.0.lock().unwrap(), 1);
        assert_eq!(*apply.0.lock().unwrap(), [passphrase.expose()]);
    }

    #[tokio::test]
    async fn usable_or_paused_peer_is_rejected() {
        let mut usable = local_only();
        usable.usable_peer_device_ids.push(DeviceId::new("peer-a"));
        let (usable_case, _, _) = use_case(Ok(usable));
        assert!(matches!(
            usable_case
                .execute(
                    &Passphrase::new("new-passphrase"),
                    &Passphrase::new("new-passphrase"),
                )
                .await
                .unwrap_err(),
            ChangeEncryptionPassphraseError::MultipleDevices
        ));

        let mut paused = local_only();
        paused.paused_peer_devices.push(PausedSpaceMember {
            device_id: DeviceId::new("peer-b"),
            reason: SpaceMemberPauseReason::RelationshipUnconfirmed,
        });
        let (paused_case, _, _) = use_case(Ok(paused));
        assert!(matches!(
            paused_case
                .execute(
                    &Passphrase::new("new-passphrase"),
                    &Passphrase::new("new-passphrase"),
                )
                .await
                .unwrap_err(),
            ChangeEncryptionPassphraseError::MultipleDevices
        ));
    }

    #[tokio::test]
    async fn inactive_local_member_is_rejected() {
        let mut inactive = local_only();
        inactive.local_member_active = false;
        let (use_case, _, _) = use_case(Ok(inactive));

        assert!(matches!(
            use_case
                .execute(
                    &Passphrase::new("new-passphrase"),
                    &Passphrase::new("new-passphrase"),
                )
                .await
                .unwrap_err(),
            ChangeEncryptionPassphraseError::MembershipRecoveryRequired
        ));
    }

    #[tokio::test]
    async fn locked_or_recovering_membership_is_rejected() {
        let (locked, _, _) = use_case(Err(CurrentSpaceMemberScopeError::Locked));
        assert!(matches!(
            locked
                .execute(
                    &Passphrase::new("new-passphrase"),
                    &Passphrase::new("new-passphrase"),
                )
                .await
                .unwrap_err(),
            ChangeEncryptionPassphraseError::Locked
        ));

        let (recovering, _, _) = use_case(Err(CurrentSpaceMemberScopeError::RecoveryRequired));
        assert!(matches!(
            recovering
                .execute(
                    &Passphrase::new("new-passphrase"),
                    &Passphrase::new("new-passphrase"),
                )
                .await
                .unwrap_err(),
            ChangeEncryptionPassphraseError::MembershipRecoveryRequired
        ));
    }
}
