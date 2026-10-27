use std::time::Duration;

use uc_core::ports::ElapsedClockPort;

/// 读取操作系统包含挂起时间的单调计数。
///
/// - Linux、Android、HarmonyOS：`CLOCK_BOOTTIME`；
/// - macOS、iOS：`CLOCK_MONOTONIC`，系统手册规定其在睡眠期间继续递增；
/// - Windows：`GetTickCount64`，按官方文档包含睡眠与休眠；
/// - 其他平台无法保证包含挂起，读数始终不可用。
pub struct SystemElapsedClock;

impl ElapsedClockPort for SystemElapsedClock {
    fn elapsed(&self) -> Option<Duration> {
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
fn read_elapsed() -> Option<Duration> {
    let mut spec = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `spec` 是有效的可写 timespec，调用不保留该指针。
    if unsafe { libc::clock_gettime(CLOCK_ID, &mut spec) } != 0 {
        return None;
    }
    let secs = u64::try_from(spec.tv_sec).ok()?;
    let nanos = u32::try_from(spec.tv_nsec)
        .ok()
        .filter(|n| *n < 1_000_000_000)?;
    Some(Duration::new(secs, nanos))
}

#[cfg(windows)]
fn read_elapsed() -> Option<Duration> {
    use windows_sys::Win32::System::SystemInformation::GetTickCount64;

    // SAFETY: 无参数、无指针的纯读取调用。
    Some(Duration::from_millis(unsafe { GetTickCount64() }))
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    windows
)))]
fn read_elapsed() -> Option<Duration> {
    None
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

    #[test]
    fn readings_are_available_and_never_go_backwards() {
        let clock = SystemElapsedClock;
        let mut last = clock.elapsed();
        assert!(last.is_some());
        for _ in 0..1000 {
            let next = clock.elapsed();
            assert!(next >= last);
            last = next;
        }
    }

    #[test]
    fn reading_advances_across_a_real_sleep() {
        let clock = SystemElapsedClock;
        let before = clock.elapsed();
        std::thread::sleep(Duration::from_millis(60));
        let spent = clock.elapsed().zip(before).map(|(a, b)| a - b);
        // Windows tick 精度约 16 ms，留出余量。
        assert!(spent.is_some_and(|d| d >= Duration::from_millis(30)));
    }
}
