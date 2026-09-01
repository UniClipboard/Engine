use async_trait::async_trait;
use uc_core::crypto::domain::{Aad, Ciphertext, Plaintext};
use uc_core::ports::security::{BlobCipherError, BlobCipherPort};

struct ContractCipher;

#[async_trait]
impl BlobCipherPort for ContractCipher {
    async fn encrypt(
        &self,
        plaintext: &Plaintext,
        _aad: &Aad,
    ) -> Result<Ciphertext, BlobCipherError> {
        Ok(Ciphertext::new(plaintext.as_bytes().to_vec()))
    }

    async fn decrypt(
        &self,
        ciphertext: &Ciphertext,
        _aad: &Aad,
    ) -> Result<Plaintext, BlobCipherError> {
        Ok(Plaintext::new(ciphertext.as_bytes().to_vec()))
    }
}

#[tokio::test]
async fn persistent_blob_cipher_callers_cannot_choose_a_space_context() {
    let cipher = ContractCipher;
    let aad = Aad::new(b"entity-aad".to_vec());
    let plaintext = Plaintext::new(b"payload".to_vec());

    let ciphertext = cipher.encrypt(&plaintext, &aad).await.unwrap();
    let opened = cipher.decrypt(&ciphertext, &aad).await.unwrap();

    assert_eq!(opened.as_bytes(), b"payload");
}
