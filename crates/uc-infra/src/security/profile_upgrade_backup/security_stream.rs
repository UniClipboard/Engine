use std::io::{self, Read, Write};

use chacha20poly1305::aead::stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::aead::{Error as AeadError, Payload};
use chacha20poly1305::XChaCha20Poly1305;
use rand::RngCore;
use zeroize::Zeroizing;

use super::super::MasterKey;

const MAGIC: &[u8; 8] = b"UCPBA001";
const CHUNK_BYTES: usize = 64 * 1024;
const TAG_BYTES: usize = 16;
const NONCE_BYTES: usize = 19;

pub(in super::super) struct ArchiveWriter<W: Write> {
    output: W,
    cipher: EncryptorBE32<XChaCha20Poly1305>,
    buffer: Zeroizing<Vec<u8>>,
}

impl<W: Write> ArchiveWriter<W> {
    pub(in super::super) fn new(mut output: W, key: &MasterKey) -> io::Result<Self> {
        let mut nonce = [0; NONCE_BYTES];
        rand::rng().fill_bytes(&mut nonce);
        output.write_all(MAGIC)?;
        output.write_all(&nonce)?;
        Ok(Self {
            output,
            cipher: EncryptorBE32::new(key.as_bytes().into(), (&nonce).into()),
            buffer: Zeroizing::new(Vec::with_capacity(CHUNK_BYTES)),
        })
    }

    pub(in super::super) fn finish(mut self) -> io::Result<W> {
        let payload = Payload {
            msg: &self.buffer,
            aad: MAGIC,
        };
        let ciphertext = self.cipher.encrypt_last(payload).map_err(crypto_error)?;
        write_frame(&mut self.output, &ciphertext)?;
        self.output.flush()?;
        Ok(self.output)
    }
}

impl<W: Write> Write for ArchiveWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        // 满块立即写出；结束时额外认证最后一个不足块（可为空）。
        let count = bytes.len().min(CHUNK_BYTES - self.buffer.len());
        self.buffer.extend_from_slice(&bytes[..count]);
        if self.buffer.len() == CHUNK_BYTES {
            let payload = Payload {
                msg: &self.buffer,
                aad: MAGIC,
            };
            let ciphertext = self.cipher.encrypt_next(payload).map_err(crypto_error)?;
            write_frame(&mut self.output, &ciphertext)?;
            self.buffer.clear();
        }
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

fn write_frame(output: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    output.write_all(&(bytes.len() as u32).to_be_bytes())?;
    output.write_all(bytes)
}

pub(in super::super) struct ArchiveReader<R: Read> {
    input: R,
    cipher: Option<DecryptorBE32<XChaCha20Poly1305>>,
    buffer: Zeroizing<Vec<u8>>,
    position: usize,
}

impl<R: Read> ArchiveReader<R> {
    pub(in super::super) fn new(mut input: R, key: &MasterKey) -> io::Result<Self> {
        let mut magic = [0; MAGIC.len()];
        input.read_exact(&mut magic)?;
        if magic != *MAGIC {
            return Err(invalid_archive());
        }
        let mut nonce = [0; NONCE_BYTES];
        input.read_exact(&mut nonce)?;
        Ok(Self {
            input,
            cipher: Some(DecryptorBE32::new(key.as_bytes().into(), (&nonce).into())),
            buffer: Zeroizing::new(Vec::new()),
            position: 0,
        })
    }

    fn next_frame(&mut self) -> io::Result<()> {
        let mut encoded_length = [0; 4];
        self.input.read_exact(&mut encoded_length)?;
        let length = u32::from_be_bytes(encoded_length) as usize;
        if !(TAG_BYTES..=CHUNK_BYTES + TAG_BYTES).contains(&length) {
            return Err(invalid_archive());
        }
        let mut ciphertext = vec![0; length];
        self.input.read_exact(&mut ciphertext)?;
        let payload = Payload {
            msg: &ciphertext,
            aad: MAGIC,
        };
        let plaintext = if length == CHUNK_BYTES + TAG_BYTES {
            self.cipher
                .as_mut()
                .ok_or_else(invalid_archive)?
                .decrypt_next(payload)
                .map_err(crypto_error)?
        } else {
            let plaintext = self
                .cipher
                .take()
                .ok_or_else(invalid_archive)?
                .decrypt_last(payload)
                .map_err(crypto_error)?;
            let mut trailing = [0; 1];
            if self.input.read(&mut trailing)? != 0 {
                return Err(invalid_archive());
            }
            plaintext
        };
        self.buffer = Zeroizing::new(plaintext);
        self.position = 0;
        Ok(())
    }
}

impl<R: Read> Read for ArchiveReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.position == self.buffer.len() && self.cipher.is_some() {
            self.next_frame()?;
        }
        let count = output.len().min(self.buffer.len() - self.position);
        output[..count].copy_from_slice(&self.buffer[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

fn crypto_error(source: AeadError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, source)
}

fn invalid_archive() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "profile backup archive is invalid",
    )
}

#[cfg(test)]
#[path = "security_stream_tests.rs"]
mod tests;
