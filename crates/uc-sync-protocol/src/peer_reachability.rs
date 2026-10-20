//! presence 固定线上字节；纯 codec 不承担准入、拨号或在线判断。

pub const PEER_REACHABILITY_ALPN: &[u8] = b"uniclipboard/presence/2";
pub const LEGACY_PEER_REACHABILITY_ALPN: &[u8] = b"uniclipboard/presence/1";
pub const ADMISSION_REQUEST: u8 = 0x01;
pub const ADMISSION_ACCEPTED: u8 = 0x01;
pub const ADMISSION_REJECTED: u8 = 0x02;
pub const FRAME_SIZE: usize = 17;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("presence frame length is invalid")]
    Length,
    #[error("presence frame type is invalid")]
    Type,
    #[error("presence challenge does not match")]
    Nonce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckReply {
    Accepted,
    Rejected,
}

pub fn request(nonce: &[u8; 16]) -> [u8; FRAME_SIZE] {
    frame(0x10, nonce)
}
pub fn reply(nonce: &[u8; 16], admitted: bool) -> [u8; FRAME_SIZE] {
    frame(if admitted { 0x11 } else { 0x12 }, nonce)
}
fn frame(kind: u8, nonce: &[u8; 16]) -> [u8; FRAME_SIZE] {
    let mut frame = [0; FRAME_SIZE];
    frame[0] = kind;
    frame[1..].copy_from_slice(nonce);
    frame
}
pub fn challenge(frame: &[u8]) -> Result<[u8; 16], FrameError> {
    if frame.len() != FRAME_SIZE {
        return Err(FrameError::Length);
    }
    if frame[0] != 0x10 {
        return Err(FrameError::Type);
    }
    let mut nonce = [0; 16];
    nonce.copy_from_slice(&frame[1..]);
    Ok(nonce)
}
pub fn classify(frame: &[u8], nonce: &[u8; 16]) -> Result<CheckReply, FrameError> {
    if frame.len() != FRAME_SIZE {
        return Err(FrameError::Length);
    }
    if frame[1..] != nonce[..] {
        return Err(FrameError::Nonce);
    }
    match frame[0] {
        0x11 => Ok(CheckReply::Accepted),
        0x12 => Ok(CheckReply::Rejected),
        _ => Err(FrameError::Type),
    }
}

#[cfg(test)]
mod tests {
    use super::{challenge, classify, reply, request, CheckReply, FrameError};

    #[test]
    fn original_presence_vectors_and_nonce_validation_are_preserved() {
        let nonce = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let original = [0x10, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let accepted = [0x11, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let rejected = [0x12, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        assert_eq!(request(&nonce), original);
        assert_eq!(reply(&nonce, true), accepted);
        assert_eq!(reply(&nonce, false), rejected);
        assert_eq!(challenge(&original), Ok(nonce));
        assert_eq!(classify(&accepted, &nonce), Ok(CheckReply::Accepted));
        assert_eq!(classify(&rejected, &nonce), Ok(CheckReply::Rejected));
        assert_eq!(classify(&rejected, &[9; 16]), Err(FrameError::Nonce));
        assert_eq!(classify(&original, &nonce), Err(FrameError::Type));
        for length in 0..17 {
            assert_eq!(challenge(&original[..length]), Err(FrameError::Length));
        }
        assert_eq!(classify(&[0x11; 18], &nonce), Err(FrameError::Length));
    }
}
