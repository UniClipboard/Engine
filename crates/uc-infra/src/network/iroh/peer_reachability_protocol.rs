//! Peer reachability wire frames and bounded exchanges on an admitted connection.

use std::time::Duration;

use iroh::endpoint::Connection;

pub(super) const ALPN: &[u8] = b"uniclipboard/presence/2";
pub(super) const LEGACY_ALPN: &[u8] = b"uniclipboard/presence/1";
pub(super) const CHECK_BUDGET: Duration = Duration::from_secs(5);
pub(super) const FRAME_SIZE: usize = 17;
const REQUEST: u8 = 0x10;
const ACCEPTED: u8 = 0x11;
const REJECTED: u8 = 0x12;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum CheckResult {
    Alive,
    Rejected,
    Failed,
    TimedOut,
}

fn frame(kind: u8, challenge: &[u8; 16]) -> [u8; FRAME_SIZE] {
    let mut bytes = [0; FRAME_SIZE];
    bytes[0] = kind;
    bytes[1..].copy_from_slice(challenge);
    bytes
}

pub(super) fn challenge(bytes: &[u8]) -> Option<[u8; 16]> {
    if bytes.len() != FRAME_SIZE || bytes[0] != REQUEST {
        return None;
    }
    bytes[1..].try_into().ok()
}

pub(super) fn reply(challenge: &[u8; 16], admitted: bool) -> [u8; FRAME_SIZE] {
    frame(if admitted { ACCEPTED } else { REJECTED }, challenge)
}

fn classify(bytes: &[u8], challenge: &[u8; 16]) -> CheckResult {
    if bytes.len() != FRAME_SIZE || bytes[1..] != challenge[..] {
        return CheckResult::Failed;
    }
    match bytes[0] {
        ACCEPTED => CheckResult::Alive,
        REJECTED => CheckResult::Rejected,
        _ => CheckResult::Failed,
    }
}

pub(super) async fn check(connection: &Connection) -> CheckResult {
    let challenge: [u8; 16] = rand::random();
    let exchange = async {
        let (mut send, mut receive) = connection.open_bi().await.ok()?;
        send.write_all(&frame(REQUEST, &challenge)).await.ok()?;
        send.finish().ok()?;
        let response = receive.read_to_end(FRAME_SIZE).await.ok()?;
        Some(classify(&response, &challenge))
    };
    match tokio::time::timeout(CHECK_BUDGET, exchange).await {
        Ok(Some(result)) => result,
        Ok(None) => CheckResult::Failed,
        Err(_) => CheckResult::TimedOut,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_challenge_and_frame_kind_are_required() {
        let nonce = [7; 16];
        assert_eq!(challenge(&frame(REQUEST, &nonce)), Some(nonce));
        assert_eq!(classify(&reply(&nonce, true), &nonce), CheckResult::Alive);
        assert_eq!(
            classify(&reply(&nonce, false), &nonce),
            CheckResult::Rejected
        );
        assert_eq!(
            classify(&reply(&[8; 16], true), &nonce),
            CheckResult::Failed
        );
        assert_eq!(
            classify(&frame(REQUEST, &nonce), &nonce),
            CheckResult::Failed
        );
        assert_eq!(challenge(&reply(&nonce, true)), None);
        for length in 0..FRAME_SIZE {
            assert_eq!(
                classify(&reply(&nonce, true)[..length], &nonce),
                CheckResult::Failed
            );
            assert_eq!(challenge(&frame(REQUEST, &nonce)[..length]), None);
        }
        let mut oversized = reply(&nonce, true).to_vec();
        oversized.push(0);
        assert_eq!(classify(&oversized, &nonce), CheckResult::Failed);
        assert_eq!(challenge(&oversized), None);
    }
}
