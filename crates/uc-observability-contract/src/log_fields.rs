//! 模块日志字段目录：每个字段名对应一个值类别，记录点只能写目录内的字段名和该类别接受的值。
//!
//! 字段名被展开为真实的模块路径（`fields::<名称>`），未登记的名字是编译错误；值类别由 [`Accept`]
//! 的实现集合决定，类别不匹配同样是编译错误。运行期文本字段白名单由本目录派生，不再另行手写。
//!
//! 迁移期间目录只包含已迁移到 `uc_*!` 宏的字段，其余字段仍在运行期 crate 的过渡清单里；
//! 字段每迁移一个就从过渡清单搬到这里。

use std::fmt::Display;
use std::io;

use tracing::field::{display, DebugValue, DisplayValue, Value};

/// 固定词表：`&'static str` 字面量，或为自己实现 `Accept<Literal>` 的封闭枚举。
pub struct Literal;
/// 应用生成的随机标识，必须经 [`id`] 显式适配。
pub struct Identifier;
/// 整数计数、字节数、时长等。
pub struct Number;
/// 布尔值。
pub struct Flag;
/// [`crate::error_source::io_error_kind`] 的结果。
pub struct IoKind;

/// 值类别在目录中的运行期表示，用于派生白名单。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldClass {
    Literal,
    Identifier,
    Number,
    Flag,
    IoKind,
}

impl FieldClass {
    /// 是否落盘文本：数字与布尔字段不受文本白名单约束。
    pub const fn is_text(self) -> bool {
        matches!(self, Self::Literal | Self::Identifier | Self::IoKind)
    }
}

/// 目录中的一个字段。
#[derive(Clone, Copy, Debug)]
pub struct CatalogField {
    pub name: &'static str,
    pub class: FieldClass,
}

/// 字段接受某一类别的值，并转成 tracing 可记录的值。
#[diagnostic::on_unimplemented(
    message = "this value is not accepted by the log field's declared class",
    label = "not accepted here",
    note = "fixed-vocabulary fields take literals or closed enums; identifier fields take `id(&value)`; \
            see `uc_observability_contract::log_fields`"
)]
pub trait Accept<Class> {
    type Out: Value;

    fn accept(self) -> Self::Out;
}

/// 固定词表：字面量直接接受；封闭枚举各自为自己实现 `Accept<Literal>`，返回固定文本。
impl Accept<Literal> for &'static str {
    type Out = &'static str;

    fn accept(self) -> &'static str {
        self
    }
}

/// 显式声明“这是应用生成的随机标识”的适配器，构造点即评审点。
pub struct Id<'a>(&'a dyn Display);

pub fn id(value: &dyn Display) -> Id<'_> {
    Id(value)
}

impl<'a> Accept<Identifier> for Id<'a> {
    type Out = DisplayValue<&'a dyn Display>;

    fn accept(self) -> Self::Out {
        display(self.0)
    }
}

macro_rules! accept_as_is {
    ($class:ty: $($value:ty),+ $(,)?) => {
        $(impl Accept<$class> for $value {
            type Out = $value;

            fn accept(self) -> $value {
                self
            }
        })+
    };
}

accept_as_is!(Number: u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize);
accept_as_is!(Flag: bool);

impl Accept<IoKind> for Option<DebugValue<io::ErrorKind>> {
    type Out = Self;

    fn accept(self) -> Self {
        self
    }
}

/// 错误字段：沿用既有错误链渲染，只接受 `&dyn Error`。
pub fn error_field<'a>(
    error: &'a (dyn std::error::Error + 'static),
) -> &'a (dyn std::error::Error + 'static) {
    error
}

#[doc(hidden)]
#[macro_export]
macro_rules! __log_field_ack {
    (Identifier(random)) => {};
    (Literal) => {};
    (Number) => {};
    (Flag) => {};
    (IoKind) => {};
}

/// 声明字段目录：每项 `名称: 类别`；`Identifier` 必须写成 `Identifier(random)`，确认取值由应用随机生成。
#[doc(hidden)]
#[macro_export]
macro_rules! __log_field_catalog {
    ($($name:ident : $class:ident $(($ack:ident))?),* $(,)?) => {
        $($crate::__log_field_ack!($class $(($ack))?);)*
        pub mod fields {
            $(pub mod $name {
                /// 该字段声明的值类别。
                pub type Class = $crate::log_fields::$class;
            })*
        }
        pub const CATALOG: &[$crate::log_fields::CatalogField] = &[
            $($crate::log_fields::CatalogField {
                name: stringify!($name),
                class: $crate::log_fields::FieldClass::$class,
            }),*
        ];
    };
}

__log_field_catalog! {
    entry_id: Identifier(random),
    error_kind: Literal,
    io_error_kind: IoKind,
}

/// 目录中该名字是否为落盘文本字段。
pub fn text_field_in_catalog(name: &str) -> bool {
    CATALOG
        .iter()
        .any(|field| field.name == name && field.class.is_text())
}
