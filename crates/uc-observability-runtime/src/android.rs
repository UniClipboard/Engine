use std::ffi::c_void;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AndroidTlsInitError {
    #[error("Android TLS context is invalid")]
    InvalidContext,
    #[error("Android TLS verifier initialization failed")]
    InitializationFailed,
}

/// 使用当前 JNI 帧初始化 Android 系统证书校验器。
///
/// # Safety
///
/// 两个指针必须来自当前线程正在执行的 JNI 方法，并在本次调用期间保持有效。
pub unsafe fn initialize_android_tls(
    raw_env: *mut c_void,
    raw_context: *mut c_void,
) -> Result<(), AndroidTlsInitError> {
    if raw_env.is_null() || raw_context.is_null() {
        return Err(AndroidTlsInitError::InvalidContext);
    }

    let mut unowned = unsafe { jni22::EnvUnowned::from_raw(raw_env.cast()) };
    let outcome = unowned.with_env(|env| -> jni22::errors::Result<()> {
        let context = unsafe { jni22::objects::JObject::from_raw(env, raw_context.cast()) };
        rustls_platform_verifier::android::init_with_env(env, context)
    });
    match outcome.into_outcome() {
        jni22::Outcome::Ok(()) => Ok(()),
        jni22::Outcome::Err(_) | jni22::Outcome::Panic(_) => {
            Err(AndroidTlsInitError::InitializationFailed)
        }
    }
}
