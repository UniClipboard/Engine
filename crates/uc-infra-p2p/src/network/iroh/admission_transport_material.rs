//! 将当前 Iroh 地址转换为准入动作所需的不透明传输材料。

use std::sync::Arc;

use iroh::Endpoint;
use uc_application::deps::{
    JoinerStartMaterialError, JoinerTransportMaterial, JoinerTransportMaterialPort,
    PrepareSponsorCandidateError, SponsorContinuationRoutePort,
};
use uc_core::membership::AdmissionContinuationRoute;
use uc_core::ports::security::IdentityFingerprintFactoryPort;

use super::space_admission::encode_space_admission_route;

/// 只保留材料采样能力，Endpoint 生命周期由节点负责。
pub struct IrohAdmissionTransportMaterial {
    endpoint: Arc<Endpoint>,
    fingerprints: Arc<dyn IdentityFingerprintFactoryPort>,
}

impl IrohAdmissionTransportMaterial {
    /// 绑定节点的同一 Endpoint，不在组装时采样地址。
    pub(super) fn new(
        endpoint: Arc<Endpoint>,
        fingerprints: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> Self {
        Self {
            endpoint,
            fingerprints,
        }
    }
}

impl JoinerTransportMaterialPort for IrohAdmissionTransportMaterial {
    /// 一次采样同时产生身份、公钥与待签名地址。
    fn prepare(&self) -> Result<JoinerTransportMaterial, JoinerStartMaterialError> {
        let address = self.endpoint.addr();
        let identity_fingerprint = self
            .fingerprints
            .from_public_key(address.id.as_bytes())
            .map_err(|source| {
                JoinerStartMaterialError::unavailable(
                    source.context("derive the endpoint identity fingerprint for Space admission"),
                )
            })?;
        let transport_address_blob =
            postcard::to_stdvec(&address).map_err(JoinerStartMaterialError::unavailable)?;
        Ok(JoinerTransportMaterial {
            identity_fingerprint,
            transport_public_key: address.id.as_bytes().to_vec(),
            transport_address_blob,
        })
    }
}

impl SponsorContinuationRoutePort for IrohAdmissionTransportMaterial {
    /// Candidate 生成时采样并编码当前继续路由。
    fn prepare(&self) -> Result<AdmissionContinuationRoute, PrepareSponsorCandidateError> {
        let route = encode_space_admission_route(&self.endpoint.addr(), None)
            .map_err(PrepareSponsorCandidateError::unavailable)?;
        AdmissionContinuationRoute::from_bytes(route).map_err(PrepareSponsorCandidateError::invalid)
    }
}
