use opaque_ke::ciphersuite::CipherSuite;
use opaque_ke::ksf::Identity;
use opaque_ke::rand::{CryptoRng, Error as RandError, RngCore};
use opaque_ke::{
    ClientRegistration, CredentialFinalization, CredentialRequest, CredentialResponse,
    ServerRegistration, TripleDh,
};
use sha2::Sha512;
use static_assertions::assert_not_impl_any;
use uc_infra::security::{
    SpaceAdmissionClientState, SpaceAdmissionContinuationCredential, SpaceAdmissionKe1,
    SpaceAdmissionKe2, SpaceAdmissionKe3, SpaceAdmissionRegistration,
    SpaceAdmissionRegistrationEncoding, SpaceAdmissionServerSetup,
    SpaceAdmissionServerSetupEncoding, SpaceAdmissionServerState,
};

assert_not_impl_any!(SpaceAdmissionServerSetup: Clone, std::fmt::Debug);
assert_not_impl_any!(SpaceAdmissionServerSetupEncoding: Clone, std::fmt::Debug);
assert_not_impl_any!(SpaceAdmissionRegistration: Clone, std::fmt::Debug);
assert_not_impl_any!(SpaceAdmissionRegistrationEncoding: Clone, std::fmt::Debug);
assert_not_impl_any!(SpaceAdmissionClientState: Clone, std::fmt::Debug);
assert_not_impl_any!(SpaceAdmissionServerState: Clone, std::fmt::Debug);
assert_not_impl_any!(SpaceAdmissionKe1: Clone, std::fmt::Debug);
assert_not_impl_any!(SpaceAdmissionKe2: Clone, std::fmt::Debug);
assert_not_impl_any!(SpaceAdmissionKe3: Clone, std::fmt::Debug);
assert_not_impl_any!(SpaceAdmissionContinuationCredential: Clone, std::fmt::Debug);

struct Rfc9807CipherSuite;

impl CipherSuite for Rfc9807CipherSuite {
    type OprfCs = opaque_ke::Ristretto255;
    type KeyExchange = TripleDh<opaque_ke::Ristretto255, Sha512>;
    type Ksf = Identity;
}

struct VectorRng {
    bytes: Vec<u8>,
}

impl VectorRng {
    fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

impl RngCore for VectorRng {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0u8; 4];
        self.fill_bytes(&mut bytes);
        u32::from_le_bytes(bytes)
    }

    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0u8; 8];
        self.fill_bytes(&mut bytes);
        u64::from_le_bytes(bytes)
    }

    fn fill_bytes(&mut self, destination: &mut [u8]) {
        let copied_len = self.bytes.len().min(destination.len());
        destination[..copied_len].copy_from_slice(&self.bytes[..copied_len]);
        self.bytes.rotate_left(copied_len);
    }

    fn try_fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), RandError> {
        self.fill_bytes(destination);
        Ok(())
    }
}

impl CryptoRng for VectorRng {}

fn decode(hexadecimal: &str) -> Vec<u8> {
    hex::decode(hexadecimal).expect("RFC 9807 vector is valid hexadecimal")
}

#[test]
fn rfc9807_appendix_c_ristretto255_sha512_vector_matches_pinned_opaque() {
    let password = decode("436f7272656374486f72736542617474657279537461706c65");
    let blind_registration =
        decode("76cfbfe758db884bebb33582331ba9f159720ca8784a2a070a265d9c2d6abe01");
    let mut registration_rng = VectorRng::new(blind_registration);
    let registration =
        ClientRegistration::<Rfc9807CipherSuite>::start(&mut registration_rng, &password)
            .expect("RFC registration request should be generated");
    assert_eq!(
        registration.message.serialize().as_slice(),
        decode("5059ff249eb1551b7ce4991f3336205bde44a105a032e747d21bf382e75f7a71")
    );

    let registration_upload = decode(concat!(
        "76a845464c68a5d2f7e442436bb1424953b17d3e2e289ccbaccafb57ac5c3675",
        "1ac5844383c7708077dea41cbefe2fa15724f449e535dd7dd562e66f5ecfb958",
        "64eadddec9db5874959905117dad40a4524111849799281fefe3c51fa82785c5",
        "ac13171b2f17bc2c74997f0fce1e1f35bec6b91fe2e12dbd323d23ba7a38dfec",
        "634b0f5b96109c198a8027da51854c35bee90d1e1c781806d07d49b76de6a28b",
        "8d9e9b6c93b9f8b64d16dddd9c5bfb5fea48ee8fd2f75012a8b308605cdd8ba5"
    ));
    let registration_record =
        ServerRegistration::<Rfc9807CipherSuite>::deserialize(&registration_upload)
            .expect("RFC registration upload should deserialize");
    assert_eq!(
        registration_record.serialize().as_slice(),
        registration_upload
    );

    let ke1_bytes = decode(concat!(
        "c4dedb0ba6ed5d965d6f250fbe554cd45cba5dfcce3ce836e4aee778aa3cd44d",
        "da7e07376d6d6f034cfa9bb537d11b8c6b4238c334333d1f0aebb380cae6a6cc",
        "6e29bee50701498605b2c085d7b241ca15ba5c32027dd21ba420b94ce60da326"
    ));
    let ke1 = CredentialRequest::<Rfc9807CipherSuite>::deserialize(&ke1_bytes)
        .expect("RFC KE1 should deserialize");
    assert_eq!(
        ke1.serialize().as_slice(),
        ke1_bytes,
        "RFC KE1 must round-trip exactly"
    );

    let ke2_bytes = decode(concat!(
        "7e308140890bcde30cbcea28b01ea1ecfbd077cff62c4def8efa075aabcbb471",
        "38fe59af0df2c79f57b8780278f5ae47355fe1f817119041951c80f612fdfc6d",
        "d6ec60bcdb26dc455ddf3e718f1020490c192d70dfc7e403981179d8073d1146",
        "a4f9aa1ced4e4cd984c657eb3b54ced3848326f70331953d91b02535af44d9f",
        "edc80188ca46743c52786e0382f95ad85c08f6afcd1ccfbff95e2bdeb015b16",
        "6c6b20b92f832cc6df01e0b86a7efd92c1c804ff865781fa93f2f20b446c837",
        "1b671cd9960ecef2fe0d0f7494986fa3d8b2bb01963537e60efb13981e138e3d4a1",
        "c4f62198a9d6fa9170c42c3c71f1971b29eb1d5d0bd733e40816c91f7912cc4",
        "a660c48dae03e57aaa38f3d0cffcfc21852ebc8b405d15bd6744945ba1a93438",
        "a162b6111699d98a16bb55b7bdddfe0fc5608b23da246e7bd73b47369169c5c90"
    ));
    let ke2 = CredentialResponse::<Rfc9807CipherSuite>::deserialize(&ke2_bytes)
        .expect("RFC KE2 should deserialize");
    assert_eq!(
        ke2.serialize().as_slice(),
        ke2_bytes,
        "RFC KE2 must round-trip exactly"
    );

    let ke3_bytes = decode(concat!(
        "4455df4f810ac31a6748835888564b536e6da5d9944dfea9e34defb9575fe5e2",
        "661ef61d2ae3929bcf57e53d464113d364365eb7d1a57b629707ca48da18e442"
    ));
    let ke3 = CredentialFinalization::<Rfc9807CipherSuite>::deserialize(&ke3_bytes)
        .expect("RFC KE3 should deserialize");
    assert_eq!(
        ke3.serialize().as_slice(),
        ke3_bytes,
        "RFC KE3 must round-trip exactly"
    );
}
