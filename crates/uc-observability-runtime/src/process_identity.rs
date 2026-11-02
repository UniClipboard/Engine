//! Kernel creation identity for a registered PID. Never persisted or logged.

use std::io;

#[cfg(target_os = "macos")]
pub(crate) fn birth_identity(pid: u32) -> io::Result<u128> {
    use std::mem::{size_of, MaybeUninit};
    let pid = i32::try_from(pid).map_err(io::Error::other)?;
    let mut information = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    // SAFETY: the kernel writes at most the supplied size into this correctly aligned buffer.
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            information.as_mut_ptr().cast(),
            size_of::<libc::proc_bsdinfo>() as i32,
        )
    };
    if read != size_of::<libc::proc_bsdinfo>() as i32 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: proc_pidinfo returned the complete initialized structure.
    let information = unsafe { information.assume_init() };
    Ok(u128::from(information.pbi_start_tvsec) * 1_000_000
        + u128::from(information.pbi_start_tvusec))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn birth_identity(pid: u32) -> io::Result<u128> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    // The comm field can contain spaces and parentheses. Numeric fields follow its final ')'.
    let (_, fields) = stat
        .rsplit_once(") ")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid process stat"))?;
    let start_ticks = fields.split_whitespace().nth(19).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing process creation ticks")
    })?;
    start_ticks.parse::<u128>().map_err(io::Error::other)
}

#[cfg(target_os = "windows")]
pub(crate) fn birth_identity(pid: u32) -> io::Result<u128> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: read-only process query; the returned handle is closed on every path below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = creation;
    let mut kernel = creation;
    let mut user = creation;
    // SAFETY: all output pointers reference live FILETIME values.
    let read = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    let result = if read == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok((u128::from(creation.dwHighDateTime) << 32) | u128::from(creation.dwLowDateTime))
    };
    // SAFETY: this function owns the non-null OpenProcess handle.
    let closed = unsafe { CloseHandle(handle) };
    if closed == 0 && result.is_ok() {
        return Err(io::Error::last_os_error());
    }
    result
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "android",
    target_os = "windows"
)))]
pub(crate) fn birth_identity(_pid: u32) -> io::Result<u128> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process creation identity unavailable",
    ))
}
