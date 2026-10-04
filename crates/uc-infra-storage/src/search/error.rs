//! 搜索适配器到 `SearchError` 端口边界的失败转换。

use uc_core::search::error::SearchError;

/// 把下层失败转换为 `SearchError::Internal`：保留完整来源，只附加固定动作说明。
pub(crate) fn internal<E>(action: &'static str) -> impl FnOnce(E) -> SearchError
where
    E: Into<anyhow::Error>,
{
    move |error| SearchError::Internal(error.into().context(action).into())
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io;

    use super::*;

    #[test]
    fn internal_keeps_lower_level_error_behind_fixed_action() {
        let error = internal("load search rows")(io::Error::from(io::ErrorKind::PermissionDenied));

        assert_eq!(error.to_string(), "internal search error");
        let action = error.source().expect("fixed action");
        assert_eq!(action.to_string(), "load search rows");
        let io_error = action
            .source()
            .and_then(|source| source.downcast_ref::<io::Error>())
            .expect("io error in source chain");
        assert_eq!(io_error.kind(), io::ErrorKind::PermissionDenied);
    }
}
