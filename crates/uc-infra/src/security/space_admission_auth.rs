use anyhow::Context;
use hkdf::Hkdf;
use opaque_ke::argon2::{Algorithm, Argon2, Params, Version};
use opaque_ke::ciphersuite::CipherSuite;
use opaque_ke::generic_array::typenum::Unsigned;
use opaque_ke::rand::rngs::OsRng;
use opaque_ke::rand::RngCore;
use opaque_ke::{
    ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialFinalization, CredentialRequest,
    CredentialResponse, ServerLogin, ServerLoginParameters, ServerRegistration,
    ServerRegistrationLen, ServerSetup, TripleDh,
};
use sha2::{Digest, Sha512};
use subtle::ConstantTimeEq;
use uc_core::crypto::domain::Passphrase;
use uc_core::membership::{
    AdmissionChannelPeerId, InvitationId, SpaceAdmissionId, SpaceAdmissionProtocolVersion,
};
use zeroize::Zeroizing;

const REGISTRATION_ENCODING_MAGIC: &[u8; 8] = b"UCOPAQRG";
const REGISTRATION_ENCODING_VERSION: u16 = 1;
const REGISTRATION_ENCODING_HEADER_LEN: usize = REGISTRATION_ENCODING_MAGIC.len() + 2 + 32;
const SERVER_SETUP_ENCODING_MAGIC: &[u8; 8] = b"UCOPAQS1";
const SERVER_SETUP_ENCODING_VERSION: u16 = 1;
const SERVER_SETUP_SERIALIZED_LEN: usize = 128;

#[derive(Debug)]
struct ContinuationCredentialExpansionError(hkdf::InvalidLength);

impl std::fmt::Display for ContinuationCredentialExpansionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OPAQUE continuation credential length is invalid")
    }
}

impl std::error::Error for ContinuationCredentialExpansionError {}

#[derive(Debug, thiserror::Error)]
enum RegistrationEncodingError {
    #[error("OPAQUE registration encoding length is invalid")]
    InvalidLength,
    #[error("OPAQUE registration encoding marker is invalid")]
    InvalidMarker,
    #[error("OPAQUE registration encoding version is unsupported")]
    UnsupportedVersion,
}

#[derive(Debug, thiserror::Error)]
enum ServerSetupEncodingError {
    #[error("OPAQUE server setup encoding length is invalid")]
    InvalidLength,
    #[error("OPAQUE server setup encoding marker is invalid")]
    InvalidMarker,
    #[error("OPAQUE server setup encoding version is unsupported")]
    UnsupportedVersion,
}

pub struct SpaceAdmissionAuth;

struct SpaceAdmissionCipherSuite;

impl CipherSuite for SpaceAdmissionCipherSuite {
    type OprfCs = opaque_ke::Ristretto255;
    type KeyExchange = TripleDh<opaque_ke::Ristretto255, Sha512>;
    type Ksf = Argon2<'static>;
}

pub struct SpaceAdmissionServerSetup(ServerSetup<SpaceAdmissionCipherSuite>);

pub struct SpaceAdmissionServerSetupEncoding(Zeroizing<Vec<u8>>);

impl SpaceAdmissionServerSetupEncoding {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Zeroizing<Vec<u8>> {
        self.0
    }
}

pub struct SpaceAdmissionRegistration {
    credential_identifier: [u8; 32],
    record: ServerRegistration<SpaceAdmissionCipherSuite>,
}

pub struct SpaceAdmissionRegistrationEncoding(Zeroizing<Vec<u8>>);

impl SpaceAdmissionRegistrationEncoding {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Zeroizing<Vec<u8>> {
        self.0
    }
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

pub struct SpaceAdmissionPasswordEquivalent(Zeroizing<[u8; 64]>);

impl SpaceAdmissionPasswordEquivalent {
    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

impl PartialEq for SpaceAdmissionContinuationCredential {
    fn eq(&self, other: &Self) -> bool {
        bool::from(self.0.as_ref().ct_eq(other.0.as_ref()))
    }
}

impl Eq for SpaceAdmissionContinuationCredential {}

#[derive(Debug, thiserror::Error)]
pub enum SpaceAdmissionAuthError {
    #[error("space admission server setup failed")]
    ServerSetup {
        #[source]
        source: anyhow::Error,
    },
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
    pub fn derive_password_equivalent(
        passphrase: &[u8],
        invitation_id: InvitationId,
    ) -> SpaceAdmissionPasswordEquivalent {
        let mut hasher = Sha512::new();
        hasher.update(b"uc-space-admission-password-equivalent-v1");
        hasher.update(invitation_id.as_bytes());
        hasher.update(passphrase);
        SpaceAdmissionPasswordEquivalent(Zeroizing::new(hasher.finalize().into()))
    }

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

    pub fn decode_registration_after_decryption(
        encoded: &[u8],
    ) -> Result<SpaceAdmissionRegistration, SpaceAdmissionAuthError> {
        decode_registration(encoded).map_err(|source| SpaceAdmissionAuthError::Registration {
            source: source.context("decode decrypted OPAQUE Space registration"),
        })
    }

    pub fn decode_server_setup_after_decryption(
        encoded: &[u8],
    ) -> Result<SpaceAdmissionServerSetup, SpaceAdmissionAuthError> {
        decode_server_setup(encoded).map_err(|source| SpaceAdmissionAuthError::ServerSetup {
            source: source.context("decode decrypted OPAQUE server setup"),
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

impl SpaceAdmissionServerSetup {
    pub fn encode_for_encryption(&self) -> SpaceAdmissionServerSetupEncoding {
        let serialized_setup = Zeroizing::new(self.0.serialize());
        let mut encoded = Zeroizing::new(Vec::with_capacity(
            SERVER_SETUP_ENCODING_MAGIC.len() + 2 + serialized_setup.len(),
        ));
        encoded.extend_from_slice(SERVER_SETUP_ENCODING_MAGIC);
        encoded.extend_from_slice(&SERVER_SETUP_ENCODING_VERSION.to_be_bytes());
        encoded.extend_from_slice(&serialized_setup);
        SpaceAdmissionServerSetupEncoding(encoded)
    }
}

impl SpaceAdmissionRegistration {
    pub fn encode_for_encryption(&self) -> SpaceAdmissionRegistrationEncoding {
        let serialized_record = Zeroizing::new(self.record.serialize());
        let mut encoded = Zeroizing::new(Vec::with_capacity(
            REGISTRATION_ENCODING_HEADER_LEN + serialized_record.len(),
        ));
        encoded.extend_from_slice(REGISTRATION_ENCODING_MAGIC);
        encoded.extend_from_slice(&REGISTRATION_ENCODING_VERSION.to_be_bytes());
        encoded.extend_from_slice(&self.credential_identifier);
        encoded.extend_from_slice(&serialized_record);
        SpaceAdmissionRegistrationEncoding(encoded)
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

fn decode_registration(encoded: &[u8]) -> anyhow::Result<SpaceAdmissionRegistration> {
    let record_len = <ServerRegistrationLen<SpaceAdmissionCipherSuite> as Unsigned>::USIZE;
    if encoded.len() != REGISTRATION_ENCODING_HEADER_LEN + record_len {
        return Err(RegistrationEncodingError::InvalidLength.into());
    }
    if &encoded[..REGISTRATION_ENCODING_MAGIC.len()] != REGISTRATION_ENCODING_MAGIC {
        return Err(RegistrationEncodingError::InvalidMarker.into());
    }

    let version_offset = REGISTRATION_ENCODING_MAGIC.len();
    let version = u16::from_be_bytes([encoded[version_offset], encoded[version_offset + 1]]);
    if version != REGISTRATION_ENCODING_VERSION {
        return Err(RegistrationEncodingError::UnsupportedVersion.into());
    }

    let credential_identifier_offset = version_offset + 2;
    let record_offset = credential_identifier_offset + 32;
    let mut credential_identifier = [0u8; 32];
    credential_identifier.copy_from_slice(&encoded[credential_identifier_offset..record_offset]);
    let record = ServerRegistration::deserialize(&encoded[record_offset..])
        .context("deserialize OPAQUE server registration")?;

    Ok(SpaceAdmissionRegistration {
        credential_identifier,
        record,
    })
}

fn decode_server_setup(encoded: &[u8]) -> anyhow::Result<SpaceAdmissionServerSetup> {
    let header_len = SERVER_SETUP_ENCODING_MAGIC.len() + 2;
    if encoded.len() != header_len + SERVER_SETUP_SERIALIZED_LEN {
        return Err(ServerSetupEncodingError::InvalidLength.into());
    }
    if &encoded[..SERVER_SETUP_ENCODING_MAGIC.len()] != SERVER_SETUP_ENCODING_MAGIC {
        return Err(ServerSetupEncodingError::InvalidMarker.into());
    }

    let version_offset = SERVER_SETUP_ENCODING_MAGIC.len();
    let version = u16::from_be_bytes([encoded[version_offset], encoded[version_offset + 1]]);
    if version != SERVER_SETUP_ENCODING_VERSION {
        return Err(ServerSetupEncodingError::UnsupportedVersion.into());
    }

    let setup = ServerSetup::deserialize(&encoded[header_len..])
        .context("deserialize OPAQUE server setup")?;
    Ok(SpaceAdmissionServerSetup(setup))
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
