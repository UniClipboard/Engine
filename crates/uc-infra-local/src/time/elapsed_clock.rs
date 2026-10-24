use std::time::Duration;

#[cfg(unix)]
use std::io;
#[cfg(windows)]
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

#[cfg(not(windows))]
use uc_application::deps::ElapsedClockRefusal;
use uc_application::deps::{ElapsedClockError, ElapsedClockPort};

/// 读取操作系统包含挂起时间的单调计数。
///
/// Linux／Android／HarmonyOS 使用 BOOTTIME；Apple 使用包含睡眠的 MONOTONIC；
/// Windows 使用 GetTickCount64。读取错误保留来源，不使用墙钟或普通 Instant 兜底。
pub struct SystemElapsedClock;

impl ElapsedClockPort for SystemElapsedClock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        read_elapsed()
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const CLOCK_ID: libc::clockid_t = libc::CLOCK_BOOTTIME;

#[cfg(any(target_os = "macos", target_os = "ios"))]
const CLOCK_ID: libc::clockid_t = libc::CLOCK_MONOTONIC;

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
))]
fn read_elapsed() -> Result<Duration, ElapsedClockError> {
    read_posix_clock(CLOCK_ID)
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
))]
fn read_posix_clock(clock: libc::clockid_t) -> Result<Duration, ElapsedClockError> {
    let mut spec = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: spec 是有效可写 timespec，调用不保留指针。
    if unsafe { libc::clock_gettime(clock, &mut spec) } != 0 {
        return Err(ElapsedClockError {
            source: Box::new(io::Error::last_os_error()),
        });
    }
    checked_elapsed(i128::from(spec.tv_sec), i128::from(spec.tv_nsec))
}

#[cfg(unix)]
fn checked_elapsed(seconds: i128, nanoseconds: i128) -> Result<Duration, ElapsedClockError> {
    let seconds = u64::try_from(seconds).map_err(|source| ElapsedClockError {
        source: Box::new(source),
    })?;
    let nanoseconds = u32::try_from(nanoseconds).map_err(|source| ElapsedClockError {
        source: Box::new(source),
    })?;
    if nanoseconds >= 1_000_000_000 {
        return Err(ElapsedClockError {
            source: Box::new(ElapsedClockRefusal::InvalidNativeCount),
        });
    }
    Ok(Duration::new(seconds, nanoseconds))
}

#[cfg(windows)]
fn read_elapsed() -> Result<Duration, ElapsedClockError> {
    // SAFETY: 无参数、无指针的纯读取调用。
    Ok(Duration::from_millis(unsafe { GetTickCount64() }))
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    windows
)))]
fn read_elapsed() -> Result<Duration, ElapsedClockError> {
    Err(ElapsedClockError {
        source: Box::new(ElapsedClockRefusal::UnsupportedPlatform),
    })
}

#[cfg(all(
    test,
    any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        windows
    )
))]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::error::Error;
    #[cfg(unix)]
    use std::num::TryFromIntError;
    #[cfg(unix)]
    use uc_core::error_class::ErrorClass;

    #[test]
    fn readings_are_available_and_never_go_backwards() {
        let clock = SystemElapsedClock;
        let mut last = clock.elapsed().expect("native elapsed clock");
        for _ in 0..1000 {
            let next = clock.elapsed().expect("native elapsed clock");
            assert!(next >= last);
            last = next;
        }
    }

    #[test]
    fn reading_advances_during_thread_sleep() {
        let clock = SystemElapsedClock;
        let before = clock.elapsed().expect("before thread sleep");
        std::thread::sleep(Duration::from_millis(60));
        let spent = clock.elapsed().expect("after thread sleep") - before;
        // 仅是线程等待，不冒充真实系统挂起；Windows tick 留出精度余量。
        assert!(spent >= Duration::from_millis(30));
    }

    #[cfg(unix)]
    #[test]
    fn actual_native_clock_failure_keeps_errno_and_error_class() {
        let error = read_posix_clock(libc::clockid_t::MAX).expect_err("invalid clock id");
        assert_eq!(error.class(), "elapsed_clock_unavailable");
        let source = error
            .source()
            .expect("native source")
            .downcast_ref::<io::Error>()
            .expect("io source");
        assert_eq!(source.raw_os_error(), Some(libc::EINVAL));
        assert_eq!(format!("{error:?}"), "elapsed clock unavailable");
    }

    #[cfg(unix)]
    #[test]
    fn malformed_native_counts_preserve_their_failure_reason() {
        for (seconds, nanoseconds) in [(-1, 0), (0, -1), (0, i128::MAX)] {
            let error = checked_elapsed(seconds, nanoseconds).expect_err("invalid integer");
            assert!(error.source().expect("source").is::<TryFromIntError>());
        }
        let error = checked_elapsed(0, 1_000_000_000).expect_err("invalid nanos");
        assert!(matches!(
            error
                .source()
                .expect("source")
                .downcast_ref::<ElapsedClockRefusal>(),
            Some(ElapsedClockRefusal::InvalidNativeCount)
        ));
    }
}
