use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::Path,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, time::sleep};

#[cfg(target_os = "macos")]
#[allow(deprecated)] // 复用已有 libc 绑定，避免仅为时钟换算增加依赖。
fn sample(pid: u32) -> Result<Value> {
    use libc::{mach_timebase_info, proc_pid_rusage, rusage_info_v2, RUSAGE_INFO_V2};
    use std::{io, mem::MaybeUninit};
    let mut usage = MaybeUninit::<rusage_info_v2>::zeroed();
    // 系统调用只写入指定版本的完整结构；成功后才能读取。
    let status = unsafe { proc_pid_rusage(pid as i32, RUSAGE_INFO_V2, usage.as_mut_ptr().cast()) };
    if status != 0 {
        return Err(io::Error::last_os_error().into());
    }
    let usage = unsafe { usage.assume_init() };
    let mut timebase = mach_timebase_info { numer: 0, denom: 0 };
    // CPU 原始计数使用 Mach 时钟单位；不能直接标记为纳秒。
    let status = unsafe { mach_timebase_info(&mut timebase) };
    ensure!(
        status == 0 && timebase.numer != 0 && timebase.denom != 0,
        "invalid Mach timebase"
    );
    let cpu_time_ns = u64::try_from(
        (u128::from(usage.ri_user_time) + u128::from(usage.ri_system_time))
            * u128::from(timebase.numer)
            / u128::from(timebase.denom),
    )?;
    Ok(
        json!({"ri_resident_size":usage.ri_resident_size,"ri_phys_footprint":usage.ri_phys_footprint,
        "ri_user_time":usage.ri_user_time,"ri_system_time":usage.ri_system_time,
        "resident_bytes":usage.ri_resident_size,"cpu_time_ns":cpu_time_ns,
        "mach_timebase_numer":timebase.numer,"mach_timebase_denom":timebase.denom,
        "process_start_abstime":usage.ri_proc_start_abstime}),
    )
}
#[cfg(not(target_os = "macos"))]
fn sample(_: u32) -> Result<Value> {
    anyhow::bail!("platform not supported")
}

#[cfg(unix)]
pub fn monotonic_ns() -> u64 {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // 单调时钟是跨进程共享的系统计时基准。
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value);
    }
    value.tv_sec as u64 * 1_000_000_000 + value.tv_nsec as u64
}

#[cfg(not(unix))]
pub fn monotonic_ns() -> u64 {
    0
}

pub async fn collect(pid: u32, output: &Path, interval_ms: u64) -> Result<()> {
    ensure!(matches!(interval_ms, 50 | 1000), "invalid sample interval");
    let mut writer = BufWriter::new(
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(output)?,
    );
    let started = Instant::now();
    let mut stdin = tokio::io::stdin();
    let mut stop = [0];
    loop {
        let mut value = sample(pid)?;
        value["monotonic_ns"] = json!(monotonic_ns());
        value["elapsed_ns"] = json!(started.elapsed().as_nanos() as u64);
        value["api"] = json!("proc_pid_rusage/RUSAGE_INFO_V2");
        serde_json::to_writer(&mut writer, &value)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        tokio::select! {
            _ = sleep(Duration::from_millis(interval_ms)) => {},
            result = stdin.read(&mut stop) => { result?; break; }
        }
    }
    Ok(())
}
