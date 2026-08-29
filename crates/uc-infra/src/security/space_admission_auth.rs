use anyhow::Context;
use hkdf::Hkdf;
use opaque_ke::argon2::{Algorithm, Argon2, Params, Version};
use opaque_ke::ciphersuite::CipherSuite;
use opaque_ke::rand::rngs::OsRng;
use opaque_ke::rand::RngCore;
use opaque_ke::{
    ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialFinalization, CredentialRequest,
    CredentialResponse, ServerLogin, ServerLoginParameters, ServerRegistration, ServerSetup,
    TripleDh,
};
use sha2::Sha512;
use subtle::ConstantTimeEq;
use uc_core::crypto::domain::Passphrase;
use uc_core::membership::{
    AdmissionChannelPeerId, InvitationId, SpaceAdmissionId, SpaceAdmissionProtocolVersion,
};
use zeroize::Zeroizing;

#[derive(Debug)]
struct ContinuationCredentialExpansionError(hkdf::InvalidLength);

impl std::fmt::Display for ContinuationCredentialExpansionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OPAQUE continuation credential length is invalid")
    }
}

impl std::error::Error for ContinuationCredentialExpansionError {}

pub struct SpaceAdmissionAuth;

struct SpaceAdmissionCipherSuite;

impl CipherSuite for SpaceAdmissionCipherSuite {
    type OprfCs = opaque_ke::Ristretto255;
    type KeyExchange = TripleDh<opaque_ke::Ristretto255, Sha512>;
    type Ksf = Argon2<'static>;
}

pub struct SpaceAdmissionServerSetup(ServerSetup<SpaceAdmissionCipherSuite>);

pub struct SpaceAdmissionRegistration {
    credential_identifier: [u8; 32],
    record: ServerRegistration<SpaceAdmissionCipherSuite>,
}

pub struct SpaceAdmissionClientState {
    state: ClientLogin<SpaceAdmissionCipherSuite>,
    passphrase: Zeroizing<Vec<u8>>,
}

pub struct SpaceAdmissionServerState(ServerLogin<SpaceAdmissionCipherSuite>);

pub struct SpaceAdmissionKe1(CredentialRequest<SpaceAdmissionCipherSuite>);

pub struct SpaceAdmissionKe2(CredentialResponse<SpaceAdmissionCipherSuite>);

pub struct SpaceAdmissionKe3(CredentialFinalization<SpaceAdmissionCipherSuite>);

pub struct SpaceAdmissionContinuationCredential(Zeroizing<[u8; 64]>);

impl PartialEq for SpaceAdmissionContinuationCredential {
    fn eq(&self, other: &Self) -> bool {
        bool::from(self.0.as_ref().ct_eq(other.0.as_ref()))
    }
}

impl Eq for SpaceAdmissionContinuationCredential {}

#[derive(Debug, thiserror::Error)]
pub enum SpaceAdmissionAuthError {
    #[error("space admission registration failed")]
    Registration {
        #[source]
        source: anyhow::Error,
    },
    #[error("space admission authentication failed")]
    Authentication {
        #[source]
        source: anyhow::Error,
    },
}

impl SpaceAdmissionAuth {
    pub fn generate_server_setup() -> SpaceAdmissionServerSetup {
        let mut rng = OsRng;
        SpaceAdmissionServerSetup(ServerSetup::<SpaceAdmissionCipherSuite>::new(&mut rng))
    }

    pub fn register(
        server_setup: &SpaceAdmissionServerSetup,
        passphrase: &Passphrase,
    ) -> Result<SpaceAdmissionRegistration, SpaceAdmissionAuthError> {
        register(server_setup, passphrase).map_err(|source| SpaceAdmissionAuthError::Registration {
            source: source.context("OPAQUE Space registration"),
        })
    }

    pub fn start_client(
        passphrase: &Passphrase,
        _context: &SpaceAdmissionAuthContext,
    ) -> Result<(SpaceAdmissionClientState, SpaceAdmissionKe1), SpaceAdmissionAuthError> {
        let mut rng = OsRng;
        let result = ClientLogin::<SpaceAdmissionCipherSuite>::start(
            &mut rng,
            passphrase.expose().as_bytes(),
        )
        .map_err(|source| SpaceAdmissionAuthError::Authentication {
            source: anyhow::Error::new(source).context("OPAQUE client authentication start"),
        })?;

        Ok((
            SpaceAdmissionClientState {
                state: result.state,
                passphrase: Zeroizing::new(passphrase.expose().as_bytes().to_vec()),
            },
            SpaceAdmissionKe1(result.message),
        ))
    }

    pub fn start_server(
        server_setup: &SpaceAdmissionServerSetup,
        registration: &SpaceAdmissionRegistration,
        context: &SpaceAdmissionAuthContext,
        ke1: SpaceAdmissionKe1,
    ) -> Result<(SpaceAdmissionServerState, SpaceAdmissionKe2), SpaceAdmissionAuthError> {
        let mut rng = OsRng;
        let context_bytes = context.encode();
        let result = ServerLogin::start(
            &mut rng,
            &server_setup.0,
            Some(registration.record.clone()),
            ke1.0,
            &registration.credential_identifier,
            ServerLoginParameters {
                context: Some(&context_bytes),
                identifiers: Default::default(),
            },
        )
        .map_err(|source| SpaceAdmissionAuthError::Authentication {
            source: anyhow::Error::new(source).context("OPAQUE server authentication start"),
        })?;

        Ok((
            SpaceAdmissionServerState(result.state),
            SpaceAdmissionKe2(result.message),
        ))
    }
}

pub struct SpaceAdmissionAuthContext {
    protocol_version: SpaceAdmissionProtocolVersion,
    admission_id: SpaceAdmissionId,
    invitation_id: InvitationId,
    joiner_peer_id: AdmissionChannelPeerId,
    sponsor_peer_id: AdmissionChannelPeerId,
}

impl SpaceAdmissionAuthContext {
    pub fn new(
        protocol_version: SpaceAdmissionProtocolVersion,
        admission_id: SpaceAdmissionId,
        invitation_id: InvitationId,
        joiner_peer_id: AdmissionChannelPeerId,
        sponsor_peer_id: AdmissionChannelPeerId,
    ) -> Self {
        Self {
            protocol_version,
            admission_id,
            invitation_id,
            joiner_peer_id,
            sponsor_peer_id,
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(196);
        encoded.extend_from_slice(b"uniclipboard/space-admission/opaque/v1");
        encoded.extend_from_slice(&self.protocol_version.as_u16().to_be_bytes());
        encoded.extend_from_slice(b"admission");
        encoded.extend_from_slice(self.admission_id.as_bytes());
        encoded.extend_from_slice(b"invitation");
        encoded.extend_from_slice(self.invitation_id.as_bytes());
        encoded.extend_from_slice(b"joiner");
        encoded.extend_from_slice(self.joiner_peer_id.as_bytes());
        encoded.extend_from_slice(b"sponsor");
        encoded.extend_from_slice(self.sponsor_peer_id.as_bytes());
        encoded.extend_from_slice(b"ristretto255-sha512-3dh-argon2id");
        encoded
    }
}

impl SpaceAdmissionClientState {
    pub fn finish(
        self,
        context: &SpaceAdmissionAuthContext,
        ke2: SpaceAdmissionKe2,
    ) -> Result<(SpaceAdmissionContinuationCredential, SpaceAdmissionKe3), SpaceAdmissionAuthError>
    {
        let mut rng = OsRng;
        let context_bytes = context.encode();
        let ksf = admission_ksf().map_err(|source| SpaceAdmissionAuthError::Authentication {
            source: source.context("OPAQUE client Argon2 parameters"),
        })?;
        let result = self
            .state
            .finish(
                &mut rng,
                &self.passphrase,
                ke2.0,
                ClientLoginFinishParameters::new(
                    Some(&context_bytes),
                    Default::default(),
                    Some(&ksf),
                ),
            )
            .map_err(|source| SpaceAdmissionAuthError::Authentication {
                source: anyhow::Error::new(source).context("OPAQUE client authentication finish"),
            })?;
        let credential = derive_continuation_credential(&result.session_key, &context_bytes)
            .map_err(|source| SpaceAdmissionAuthError::Authentication {
                source: source.context("OPAQUE client continuation derivation"),
            })?;

        Ok((credential, SpaceAdmissionKe3(result.message)))
    }
}

impl SpaceAdmissionServerState {
    pub fn finish(
        self,
        context: &SpaceAdmissionAuthContext,
        ke3: SpaceAdmissionKe3,
    ) -> Result<SpaceAdmissionContinuationCredential, SpaceAdmissionAuthError> {
        let context_bytes = context.encode();
        let result = self
            .0
            .finish(
                ke3.0,
                ServerLoginParameters {
                    context: Some(&context_bytes),
                    identifiers: Default::default(),
                },
            )
            .map_err(|source| SpaceAdmissionAuthError::Authentication {
                source: anyhow::Error::new(source).context("OPAQUE server authentication finish"),
            })?;

        derive_continuation_credential(&result.session_key, &context_bytes).map_err(|source| {
            SpaceAdmissionAuthError::Authentication {
                source: source.context("OPAQUE server continuation derivation"),
            }
        })
    }
}

fn register(
    server_setup: &SpaceAdmissionServerSetup,
    passphrase: &Passphrase,
) -> anyhow::Result<SpaceAdmissionRegistration> {
    let mut rng = OsRng;
    let client_start = ClientRegistration::<SpaceAdmissionCipherSuite>::start(
        &mut rng,
        passphrase.expose().as_bytes(),
    )
    .context("start OPAQUE client registration")?;
    let mut credential_identifier = [0u8; 32];
    rng.fill_bytes(&mut credential_identifier);
    let server_start = ServerRegistration::<SpaceAdmissionCipherSuite>::start(
        &server_setup.0,
        client_start.message,
        &credential_identifier,
    )
    .context("start OPAQUE server registration")?;
    let ksf = admission_ksf().context("construct OPAQUE Argon2 parameters")?;
    let client_finish = client_start
        .state
        .finish(
            &mut rng,
            passphrase.expose().as_bytes(),
            server_start.message,
            ClientRegistrationFinishParameters::new(Default::default(), Some(&ksf)),
        )
        .context("finish OPAQUE client registration")?;

    Ok(SpaceAdmissionRegistration {
        credential_identifier,
        record: ServerRegistration::finish(client_finish.message),
    })
}

fn admission_ksf() -> anyhow::Result<Argon2<'static>> {
    let params = Params::new(65_536, 3, 4, Some(64)).context("construct Argon2 parameters")?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

fn derive_continuation_credential(
    session_key: &[u8],
    context: &[u8],
) -> anyhow::Result<SpaceAdmissionContinuationCredential> {
    let hkdf = Hkdf::<Sha512>::new(None, session_key);
    let mut credential = Zeroizing::new([0u8; 64]);
    let mut info = Vec::with_capacity(64 + context.len());
    info.extend_from_slice(b"uniclipboard/space-admission/continuation/v1");
    info.extend_from_slice(context);
    hkdf.expand(&info, credential.as_mut())
        .map_err(ContinuationCredentialExpansionError)
        .map_err(anyhow::Error::new)
        .context("expand OPAQUE continuation credential")?;
    Ok(SpaceAdmissionContinuationCredential(credential))
}
