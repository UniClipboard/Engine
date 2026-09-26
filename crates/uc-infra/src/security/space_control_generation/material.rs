use std::collections::BTreeSet;

use uc_application::deps::AdmissionSpaceTransitionPreparationV2;
use uc_core::membership::{
    AdmissionContentKeyCatalogV1, ContentKeyId, GroupEpoch, ProtectionGroupId, SpaceKeyMaterial,
    SpaceKeyState,
};

use super::{
    inconsistent, inconsistent_input, ActiveRuntimeManifestV3, AdmissionInputIssue,
    SpaceControlGenerationError,
};
use crate::space::import_admission_content_key_catalog;

/// 加入方目标控制世代的安全材料与准入凭据。成员记录与成员读模型不在此写入：目标世代生效后由
/// 成员状态负责人按加入后历史建立。
pub(super) struct PreparedAdmissionControl {
    space_id: uc_core::ids::SpaceId,
    security_material: SpaceKeyMaterial,
    credentials: Vec<u8>,
}

impl PreparedAdmissionControl {
    pub(super) fn try_from_input(
        input: &AdmissionSpaceTransitionPreparationV2,
        manifest: &ActiveRuntimeManifestV3,
    ) -> Result<Self, SpaceControlGenerationError> {
        input
            .target_security_commitment
            .validate()
            .map_err(|source| {
                inconsistent_input(AdmissionInputIssue::SecurityMaterial, source.into())
            })?;
        let space_id = uc_core::ids::SpaceId::from_str(&input.target_space_id);
        if manifest.layout().space_id() != &space_id
            || input.target_security_commitment.attempt_id != *input.attempt_id.as_bytes()
            || input.target_security_commitment.lineage_id != input.target_space_id
            || input.target_security_state.is_empty()
            || input.target_admission_credentials.is_empty()
        {
            return Err(inconsistent(anyhow::anyhow!(
                "admission control material is incomplete"
            )));
        }
        let catalog =
            AdmissionContentKeyCatalogV1::decode(&input.target_key_catalog).map_err(|source| {
                inconsistent_input(AdmissionInputIssue::SecurityMaterial, source.into())
            })?;
        if catalog.target_epoch != input.target_security_commitment.target_epoch
            || catalog.digest() != input.target_security_commitment.key_catalog_digest
        {
            return Err(inconsistent_input(
                AdmissionInputIssue::SecurityMaterial,
                anyhow::anyhow!("admission content catalog does not match commitment"),
            ));
        }
        let current_content_key_id = ContentKeyId::from_string(&catalog.current_content_key_id)
            .map_err(|source| {
                inconsistent_input(AdmissionInputIssue::SecurityMaterial, source.into())
            })?;
        let protection_group_id = ProtectionGroupId::from_string(&input.target_protection_group_id)
            .map_err(|source| {
                inconsistent_input(AdmissionInputIssue::SecurityMaterial, source.into())
            })?;
        let state = SpaceKeyState::ready_for_admission(
            space_id.clone(),
            GroupEpoch::new(catalog.target_epoch),
            current_content_key_id,
            protection_group_id,
        )
        .map_err(|source| {
            inconsistent_input(AdmissionInputIssue::SecurityMaterial, source.into())
        })?;
        let key_catalog = import_admission_content_key_catalog(&catalog).map_err(|source| {
            inconsistent_input(AdmissionInputIssue::SecurityMaterial, source.into())
        })?;
        let mut security_material =
            SpaceKeyMaterial::new(state, input.target_security_state.clone(), key_catalog, 0);
        validate_updates(input)?;
        security_material.add_pending_group_updates(input.relayed_group_updates.clone(), 0);

        if !input
            .target_relationships
            .iter()
            .any(|facts| facts.device_id == input.local_device_id)
        {
            return Err(inconsistent_input(
                AdmissionInputIssue::Relationships,
                anyhow::anyhow!("local relationship is missing"),
            ));
        }
        Ok(Self {
            space_id,
            security_material,
            credentials: input.target_admission_credentials.clone(),
        })
    }

    pub(super) fn space_id(&self) -> &uc_core::ids::SpaceId {
        &self.space_id
    }

    pub(super) fn security_material(&self) -> &SpaceKeyMaterial {
        &self.security_material
    }

    pub(super) fn credentials(&self) -> &[u8] {
        &self.credentials
    }
}

fn validate_updates(
    input: &AdmissionSpaceTransitionPreparationV2,
) -> Result<(), SpaceControlGenerationError> {
    let mut relationship_devices = BTreeSet::new();
    for facts in &input.target_relationships {
        if !relationship_devices.insert(facts.device_id.clone()) {
            return Err(inconsistent_input(
                AdmissionInputIssue::Relationships,
                anyhow::anyhow!("control relationship is duplicated"),
            ));
        }
    }
    let mut update_ids = BTreeSet::new();
    let mut recipients = BTreeSet::new();
    if input.relayed_group_updates.iter().any(|update| {
        update.recipient() == &input.local_device_id
            || !relationship_devices.contains(update.recipient())
            || update.update_id().is_empty()
            || update.payload().is_empty()
            || !update_ids.insert(update.update_id())
            || !recipients.insert(update.recipient().clone())
    }) {
        return Err(inconsistent_input(
            AdmissionInputIssue::SecurityMaterial,
            anyhow::anyhow!("control group updates are inconsistent"),
        ));
    }
    Ok(())
}
