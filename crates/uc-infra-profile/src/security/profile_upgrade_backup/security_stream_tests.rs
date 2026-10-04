use std::io::{self, Cursor, Read, Write};

use super::{ArchiveReader, ArchiveWriter};
use crate::security::MasterKey;

#[test]
fn stream_authenticates_exact_boundaries_and_rejects_truncation_reordering_and_append() {
    let key = MasterKey::from_bytes(&[9; 32]).unwrap();
    for size in [0, 1, 65_535, 65_536, 65_537, 131_072] {
        let plaintext = vec![6; size];
        let mut writer = ArchiveWriter::new(Vec::new(), &key).unwrap();
        for part in plaintext.chunks(317) {
            writer.write_all(part).unwrap();
        }
        let bytes = writer.finish().unwrap();
        let mut recovered = Vec::new();
        ArchiveReader::new(Cursor::new(&bytes), &key)
            .unwrap()
            .read_to_end(&mut recovered)
            .unwrap();
        assert_eq!(plaintext, recovered);
        for length in [0, 8, 27, bytes.len() - 1] {
            assert!(decode(&bytes[..length], &key).is_err());
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(decode(&extra, &key).is_err());
        assert!(decode(&bytes, &MasterKey::from_bytes(&[10; 32]).unwrap()).is_err());
        if size == 131_072 {
            let mut reordered = bytes.clone();
            let length = 4 + 65_536 + 16;
            reordered[27..27 + length].copy_from_slice(&bytes[27 + length..27 + 2 * length]);
            assert!(decode(&reordered, &key).is_err());
        }
    }
}

fn decode(bytes: &[u8], key: &MasterKey) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    ArchiveReader::new(Cursor::new(bytes), key)?.read_to_end(&mut output)?;
    Ok(output)
}

#[test]
fn oversized_ciphertext_frame_is_rejected_without_large_allocation() {
    let key = MasterKey::from_bytes(&[9; 32]).unwrap();
    let mut bytes = ArchiveWriter::new(Vec::new(), &key)
        .unwrap()
        .finish()
        .unwrap();
    bytes[27..31].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(decode(&bytes, &key).is_err());
}

#[test]
fn streaming_large_input_uses_bounded_output_frames_and_preserves_write_failure() {
    #[derive(Default)]
    struct BoundedSink {
        largest_write: usize,
        bytes: usize,
    }
    impl Write for BoundedSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.largest_write = self.largest_write.max(bytes.len());
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let key = MasterKey::from_bytes(&[9; 32]).unwrap();
    let mut writer = ArchiveWriter::new(BoundedSink::default(), &key).unwrap();
    let size = 16 * 1024 * 1024;
    io::copy(&mut io::repeat(19).take(size), &mut writer).unwrap();
    let output = writer.finish().unwrap();
    assert!(output.bytes > size as usize);
    assert!(output.largest_write <= 65_536 + 16);

    struct FailingSink;
    impl Write for FailingSink {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::StorageFull))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let error = match ArchiveWriter::new(FailingSink, &key) {
        Ok(_) => panic!("storage failure must be returned"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), io::ErrorKind::StorageFull);
}
