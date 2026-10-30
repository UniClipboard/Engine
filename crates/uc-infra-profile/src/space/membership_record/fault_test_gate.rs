//! 仅供独立进程故障测试在已提交的恢复检查点暂停。
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::{io::Write, os::unix::net::UnixStream};
use uc_application::deps::{MembershipLedgerError, MembershipRecord};
#[cfg(unix)]
use zeroize::Zeroizing;

pub(super) fn after_commit(record: &MembershipRecord) -> Result<(), MembershipLedgerError> {
    let Some(root) = std::env::var_os("UC_BRANCH_RECOVERY_TEST_GATE") else {
        return Ok(());
    };
    let root = PathBuf::from(root);
    let phase = match fs::read_to_string(root.join("arm")) {
        Ok(phase) => phase,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(MembershipLedgerError::unavailable_from(error)),
    };
    let MembershipRecord::Space(space) = record else {
        return Ok(());
    };
    let matching = space
        .branch_recovery
        .recovery_sessions
        .values()
        .find(|session| match phase.trim() {
            "recipient_prepared" => session.recipient_preparation().is_some(),
            "recipient_completed" => session.recipient_completion().is_some(),
            "target_prepared" => session.target_preparation().is_some(),
            "target_committed" => session.target_completion().is_some(),
            _ => false,
        });
    let Some(session) = matching else {
        return Ok(());
    };
    fs::write(root.join("reached"), b"committed")
        .map_err(MembershipLedgerError::unavailable_from)?;
    #[cfg(unix)]
    if root.join("capture.sock").exists() {
        let package = session
            .target_completion()
            .map(|(_, package)| package)
            .or_else(|| session.recipient_completion().map(|(_, package)| package))
            .ok_or_else(MembershipLedgerError::unavailable)?;
        let encoded = Zeroizing::new(
            serde_json::to_vec(package).map_err(MembershipLedgerError::unavailable_from)?,
        );
        let mut stream = UnixStream::connect(root.join("capture.sock"))
            .map_err(MembershipLedgerError::unavailable_from)?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(MembershipLedgerError::unavailable_from)?;
        stream
            .write_all(encoded.as_slice())
            .map_err(MembershipLedgerError::unavailable_from)?;
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    while !root.join("release").exists() {
        if Instant::now() >= deadline {
            return Err(MembershipLedgerError::unavailable_from(
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "test checkpoint gate timed out",
                ),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
