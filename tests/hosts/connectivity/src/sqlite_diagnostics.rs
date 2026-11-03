//! 冻结旧库也使用同一测试宿主；只采集数字错误码，不读取 SQLite 的错误文本。

use std::ffi::{c_char, c_int, c_void};
use std::io::{self, Write};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{ensure, Result};
use libsqlite3_sys::{sqlite3_config, SQLITE_CONFIG_LOG, SQLITE_OK};

static RECORDS: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn record(_context: *mut c_void, code: c_int, _message: *const c_char) {
    if RECORDS
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            (count < 64).then_some(count + 1)
        })
        .is_ok()
    {
        let _ = writeln!(io::stderr().lock(), "uc_test_sqlite_code={code}");
    }
}

pub(crate) fn install() -> Result<()> {
    // 此时尚未打开任何数据库。回调只使用静态状态，不解引用 SQLite 提供的指针。
    let code = unsafe {
        sqlite3_config(
            SQLITE_CONFIG_LOG,
            record as unsafe extern "C" fn(*mut c_void, c_int, *const c_char),
            ptr::null_mut::<c_void>(),
        )
    };
    ensure!(
        code == SQLITE_OK,
        "SQLite diagnostic setup failed (code {code})"
    );
    Ok(())
}
