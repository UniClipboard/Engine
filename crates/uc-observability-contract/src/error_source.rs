//! 日志失败原因的固定分类：沿 source chain 逐层 `downcast`，不输出任何错误正文。
//!
//! 记录点在调用处写固定的 `error_kind` 字面量，再用本模块从来源链提取下层分类；
//! 链上找不到对应类型时字段省略，不回退到正文。

use std::error::Error;
use std::io;
use std::iter::successors;

use tracing::field::{debug, DebugValue};

/// 沿 source chain（含最外层）查找第一个 `T` 类型的错误。
pub fn find_source<'a, T>(error: &'a (dyn Error + 'static)) -> Option<&'a T>
where
    T: Error + 'static,
{
    successors(Some(error), |&current| current.source()).find_map(|current| current.downcast_ref())
}

/// 返回来源链上第一个 `std::io::Error` 的 `ErrorKind`，作为日志字段 `io_error_kind` 的值。
///
/// 值为 `ErrorKind` 的变体名（如 `PermissionDenied`），与既有本地诊断记录一致；链上没有 IO 错误时返回
/// `None`，tracing 会省略该字段。`anyhow::Error` 与装箱错误以 `.as_ref()` 传入。
pub fn io_error_kind(error: &(dyn Error + 'static)) -> Option<DebugValue<io::ErrorKind>> {
    find_source::<io::Error>(error).map(|io| debug(io.kind()))
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;

    #[derive(Debug)]
    struct Wrapper(io::Error);

    impl fmt::Display for Wrapper {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("wrapper")
        }
    }

    impl Error for Wrapper {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    fn kind_text(error: &(dyn Error + 'static)) -> Option<String> {
        io_error_kind(error).map(|kind| format!("{kind:?}"))
    }

    #[test]
    fn io_error_kind_finds_nested_io_error() {
        let error = Wrapper(io::Error::from(io::ErrorKind::PermissionDenied));

        assert_eq!(kind_text(&error).as_deref(), Some("PermissionDenied"));
    }

    #[test]
    fn io_error_kind_walks_anyhow_context_chain() {
        let error = anyhow::Error::new(io::Error::from(io::ErrorKind::StorageFull))
            .context("write spool file");

        assert_eq!(kind_text(error.as_ref()).as_deref(), Some("StorageFull"));
    }

    #[test]
    fn io_error_kind_is_absent_without_io_source() {
        let error = anyhow::anyhow!("no io source");

        assert_eq!(kind_text(error.as_ref()), None);
    }

    #[test]
    fn find_source_returns_outermost_match() {
        let error = Wrapper(io::Error::from(io::ErrorKind::NotFound));

        assert!(find_source::<Wrapper>(&error).is_some());
        assert_eq!(
            find_source::<io::Error>(&error).map(io::Error::kind),
            Some(io::ErrorKind::NotFound)
        );
    }
}
