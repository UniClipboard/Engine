//! 模块日志写入口：`uc_trace!`、`uc_debug!`、`uc_info!`、`uc_warn!`、`uc_error!`。
//!
//! 写法与 `tracing` 一致：可选的 `target: 表达式,`，随后是 `字段 = 值,`，最后是字面量消息。
//! 字段名必须在 [`crate::log_fields`] 目录内，值必须是该字段类别接受的类型；`error = &e as &dyn Error`
//! 是唯一特例。宏展开为 `tracing::event!`，事件的模块路径与源码位置属于调用方。

#[doc(hidden)]
#[macro_export]
macro_rules! __uc_log {
    ($level:ident [$($target:tt)*] [$($fields:tt)*] error = $value:expr, $($rest:tt)+) => {
        $crate::__uc_log!(
            $level [$($target)*]
            [$($fields)* error = $crate::log_fields::error_field($value),]
            $($rest)+
        )
    };
    ($level:ident [$($target:tt)*] [$($fields:tt)*] $name:ident = $value:expr, $($rest:tt)+) => {
        $crate::__uc_log!(
            $level [$($target)*]
            [$($fields)* $name = $crate::log_fields::Accept::<$crate::log_fields::fields::$name::Class>::accept($value),]
            $($rest)+
        )
    };
    ($level:ident [$($target:tt)*] [$($fields:tt)*] $message:literal) => {
        $crate::__tracing::event!($($target)* $crate::__tracing::Level::$level, $($fields)* $message)
    };
}

#[macro_export]
macro_rules! uc_trace {
    (target: $target:expr, $($rest:tt)+) => { $crate::__uc_log!(TRACE [target: $target,] [] $($rest)+) };
    ($($rest:tt)+) => { $crate::__uc_log!(TRACE [] [] $($rest)+) };
}

#[macro_export]
macro_rules! uc_debug {
    (target: $target:expr, $($rest:tt)+) => { $crate::__uc_log!(DEBUG [target: $target,] [] $($rest)+) };
    ($($rest:tt)+) => { $crate::__uc_log!(DEBUG [] [] $($rest)+) };
}

#[macro_export]
macro_rules! uc_info {
    (target: $target:expr, $($rest:tt)+) => { $crate::__uc_log!(INFO [target: $target,] [] $($rest)+) };
    ($($rest:tt)+) => { $crate::__uc_log!(INFO [] [] $($rest)+) };
}

#[macro_export]
macro_rules! uc_warn {
    (target: $target:expr, $($rest:tt)+) => { $crate::__uc_log!(WARN [target: $target,] [] $($rest)+) };
    ($($rest:tt)+) => { $crate::__uc_log!(WARN [] [] $($rest)+) };
}

#[macro_export]
macro_rules! uc_error {
    (target: $target:expr, $($rest:tt)+) => { $crate::__uc_log!(ERROR [target: $target,] [] $($rest)+) };
    ($($rest:tt)+) => { $crate::__uc_log!(ERROR [] [] $($rest)+) };
}
