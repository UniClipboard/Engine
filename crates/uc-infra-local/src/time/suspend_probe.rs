//! 人工物理挂起探针；普通线程等待和进程暂停不能产生包含／排除睡眠时钟的差值。
#![cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    windows
))]

#[cfg(unix)]
use super::read_posix_clock;
use super::{ElapsedClockPort, SystemElapsedClock};
use serde_json::json;
use std::error::Error;
use std::fs;
#[cfg(windows)]
use std::io;
use std::io::ErrorKind;
use std::time::Duration;
use uc_core::file_transfer::{BudgetStatus, ElapsedBudget};
#[cfg(windows)]
use windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime;

struct Sample {
    inclusive: Duration,
    awake_before: Duration,
    awake_after: Duration,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn awake() -> Result<Duration, Box<dyn Error>> {
    Ok(read_posix_clock(libc::CLOCK_MONOTONIC)?)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn awake() -> Result<Duration, Box<dyn Error>> {
    Ok(read_posix_clock(libc::CLOCK_UPTIME_RAW)?)
}

#[cfg(windows)]
fn awake() -> Result<Duration, Box<dyn Error>> {
    let mut ticks = 0;
    // SAFETY: ticks是有效可写u64，系统调用不保留指针。
    if unsafe { QueryUnbiasedInterruptTime(&mut ticks) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(Duration::new(
        ticks / 10_000_000,
        ((ticks % 10_000_000) * 100) as u32,
    ))
}

fn sample() -> Result<Sample, Box<dyn Error>> {
    let awake_before = awake()?;
    let inclusive = SystemElapsedClock.elapsed()?;
    let awake_after = awake()?;
    assert!(awake_after >= awake_before, "awake clock regressed");
    Ok(Sample {
        inclusive,
        awake_before,
        awake_after,
    })
}

fn suspended_lower_bound(start: &Sample, end: &Sample) -> Duration {
    // 把跨时钟采样窗口全部计作清醒时间，只能低估挂起时间；不依赖墙钟或人工声明。
    let inclusive = end
        .inclusive
        .checked_sub(start.inclusive)
        .expect("elapsed clock regressed");
    let awake = end
        .awake_after
        .checked_sub(start.awake_before)
        .expect("awake clock regressed");
    inclusive.saturating_sub(awake)
}

#[test]
fn thread_sleep_does_not_count_as_system_suspend() {
    let start = sample().unwrap();
    std::thread::sleep(Duration::from_millis(60));
    let end = sample().unwrap();
    assert!(suspended_lower_bound(&start, &end) < Duration::from_millis(500));
}

#[test]
fn sampling_window_only_reduces_reported_suspend() {
    let start = Sample {
        inclusive: Duration::from_secs(100),
        awake_before: Duration::from_secs(40),
        awake_after: Duration::from_secs(41),
    };
    let end = Sample {
        inclusive: Duration::from_secs(130),
        awake_before: Duration::from_secs(44),
        awake_after: Duration::from_secs(45),
    };
    assert_eq!(suspended_lower_bound(&start, &end), Duration::from_secs(25));
}

#[test]
#[ignore = "需要操作者实际挂起系统至少10秒，不能在普通CI中记为通过"]
fn physical_suspend_consumes_original_elapsed_budgets() -> Result<(), Box<dyn Error>> {
    let report = std::env::var("UC_ELAPSED_SUSPEND_REPORT")?;
    // 清除旧成功文件；超时或读取失败后不能残留本轮成功证据。
    if let Err(error) = fs::remove_file(&report) {
        if error.kind() != ErrorKind::NotFound {
            return Err(error.into());
        }
    }
    let start = sample()?;
    let mut budgets = [5, 60, 600]
        .map(|seconds| ElapsedBudget::start(Some(start.inclusive), Duration::from_secs(seconds)));
    let mut previous_inclusive = start.inclusive;
    let mut previous_awake = start.awake_after;
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let end = sample()?;
        assert!(
            end.inclusive >= previous_inclusive,
            "elapsed clock regressed"
        );
        assert!(end.awake_before >= previous_awake, "awake clock regressed");
        previous_inclusive = end.inclusive;
        previous_awake = end.awake_after;
        let suspended = suspended_lower_bound(&start, &end);
        if suspended >= Duration::from_secs(10) {
            let states = budgets
                .iter_mut()
                .map(|budget| budget.apply(Some(end.inclusive)))
                .collect::<Vec<_>>();
            assert_eq!(states[0], BudgetStatus::Expired);
            fs::write(
                report,
                serde_json::to_vec_pretty(&json!({
                    "physical_suspend_observed": true,
                    "os": std::env::consts::OS,
                    "arch": std::env::consts::ARCH,
                    "elapsed_millis": (end.inclusive - start.inclusive).as_millis(),
                    "awake_millis_upper_bound": (end.awake_after - start.awake_before).as_millis(),
                    "suspend_millis_lower_bound": suspended.as_millis(),
                    "original_5_second_budget_expired": states[0] == BudgetStatus::Expired,
                    "original_60_second_budget_expired": states[1] == BudgetStatus::Expired,
                    "original_600_second_budget_expired": states[2] == BudgetStatus::Expired
                }))?,
            )?;
            return Ok(());
        }
        assert!(
            end.awake_after - start.awake_before < Duration::from_secs(180),
            "no physical suspend observed within 180 awake seconds"
        );
    }
}
