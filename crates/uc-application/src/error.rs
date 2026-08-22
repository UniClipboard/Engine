macro_rules! anyhow_error_constructor {
    ($fn_name:ident, $variant:ident) => {
        pub(crate) fn $fn_name<E>(source: E) -> Self
        where
            E: Into<anyhow::Error>,
        {
            Self::$variant {
                source: source.into(),
            }
        }
    };
}

pub(crate) use anyhow_error_constructor;
