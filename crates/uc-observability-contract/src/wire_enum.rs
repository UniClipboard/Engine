//! 固定词表枚举的单一来源：变体与其线上取值由同一份列表生成。

/// 声明只含单元变体的固定词表枚举，并生成 `wire()` 返回每个变体的线上取值。
///
/// - `enum Name: Type { Variant => value, .. }`：只生成枚举与 `wire()`，`Type` 通常是
///   `&'static str`，也可以是 `(&'static str, &'static str)` 这类固定元组。
/// - `serde enum Name { Variant => "value", .. }`：额外让 `Serialize`/`Deserialize` 使用同一个取值，
///   调用方自行在 `#[derive]` 中加入它们；不要再手写 `rename_all` 或 `rename`。
///
/// `wire()` 只在定义所在 crate 内可见，不会扩大公开接口。
#[macro_export]
macro_rules! wire_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident: $ty:ty {
            $($(#[$doc:meta])* $variant:ident => $wire:expr,)+
        }
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $($(#[$doc])* $variant,)+
        }

        impl $name {
            /// 该变体的线上取值。
            #[allow(dead_code, reason = "Generated vocabulary accessor; some enums only use serde")]
            pub(crate) const fn wire(self) -> $ty {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }
        }
    };
    (
        serde
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$doc:meta])* $variant:ident => $wire:literal,)+
        }
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $($(#[$doc])* #[serde(rename = $wire)] $variant,)+
        }

        impl $name {
            /// 该变体的线上取值，与序列化结果相同。
            #[allow(dead_code, reason = "Generated vocabulary accessor; some enums only use serde")]
            pub(crate) const fn wire(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    wire_enum! {
        serde
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        enum Sample {
            First => "first",
            SecondPart => "second_part",
        }
    }

    wire_enum! {
        #[derive(Clone, Copy)]
        enum Pair: (&'static str, &'static str) {
            Only => ("stage", "reason"),
        }
    }

    #[test]
    fn serde_name_and_wire_value_come_from_one_entry() {
        for variant in [Sample::First, Sample::SecondPart] {
            let json = serde_json::to_string(&variant).expect("serialize");
            assert_eq!(json, format!("\"{}\"", variant.wire()));
            let back: Sample = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, variant);
        }
    }

    #[test]
    fn tuple_values_are_supported() {
        assert_eq!(Pair::Only.wire(), ("stage", "reason"));
    }
}
