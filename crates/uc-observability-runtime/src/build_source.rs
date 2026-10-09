//! 本次编译的 Engine 源码来源。
//!
//! 来源由 build script 编译为独立的原生静态库，而不是编入本 crate 的代码。默认随 rlib 打包；显式提供来源的
//! 托管构建（CI）不打包，只在最终链接时进入可执行文件、测试与动态库，本 crate 的 rlib 因此跨提交不变，
//! 下游仍可命中编译缓存。取舍见 build.rs。

/// 与 build script 生成的 C 数组长度一致。
const LENGTH: usize = 64;
const UNKNOWN: &str = "unknown";

extern "C" {
    /// build script 生成的只读数据：`<提交号> <状态>`，NUL 结尾，其余补零。
    static uc_observability_build_source_v1: [u8; LENGTH];
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BuildSource {
    pub(crate) commit: &'static str,
    pub(crate) state: &'static str,
}

pub(crate) fn build_source() -> BuildSource {
    // SAFETY: 该符号只由本 crate 的 build script 定义为同名、长度为 LENGTH 的只读 unsigned char
    // 数组，并随本 crate 链接；缺少定义时最终产物链接失败，不会读到其它数据。内容仍逐项校验。
    let raw: &'static [u8; LENGTH] = unsafe { &uc_observability_build_source_v1 };
    parse(raw)
}

fn parse(raw: &'static [u8]) -> BuildSource {
    let text = raw
        .iter()
        .position(|byte| *byte == 0)
        .and_then(|end| std::str::from_utf8(&raw[..end]).ok())
        .unwrap_or_default();
    let (commit, state) = text.split_once(' ').unwrap_or_default();
    BuildSource {
        commit: if valid_commit(commit) {
            commit
        } else {
            UNKNOWN
        },
        state: if matches!(state, "clean" | "modified") {
            state
        } else {
            UNKNOWN
        },
    }
}

fn valid_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::{build_source, parse, BuildSource};

    const COMMIT: &str = "0292e14735dc0dabbc25269ecdbc43666f19c699";

    #[test]
    fn complete_commit_and_state_are_reported() {
        assert_eq!(
            parse(b"0292e14735dc0dabbc25269ecdbc43666f19c699 modified\0\0\0"),
            BuildSource {
                commit: COMMIT,
                state: "modified"
            }
        );
    }

    #[test]
    fn missing_source_is_reported_as_unknown() {
        assert_eq!(
            parse(b"unknown unknown\0"),
            BuildSource {
                commit: "unknown",
                state: "unknown"
            }
        );
    }

    #[test]
    fn malformed_data_never_reports_a_partial_or_unterminated_value() {
        for raw in [
            &b"0292e147 clean\0"[..],
            b"0292e14735dc0dabbc25269ecdbc43666f19c699 clean",
            b"\xff\xfe clean\0",
            b"",
        ] {
            assert_eq!(parse(raw).commit, "unknown", "{raw:?}");
        }
        assert_eq!(
            parse(b"0292e14735dc0dabbc25269ecdbc43666f19c699 dirty\0"),
            BuildSource {
                commit: COMMIT,
                state: "unknown"
            }
        );
    }

    #[test]
    fn linked_source_is_well_formed() {
        let source = build_source();
        assert!(source.commit == "unknown" || source.commit.len() == 40);
        assert!(matches!(source.state, "clean" | "modified" | "unknown"));
    }
}
