//! 锚点清单与期望登记：矩阵单元的版本、应用版本号与登记的期望结果。

use serde::Deserialize;
use serde_json::Value;

const ANCHORS: &str = include_str!("../anchors.json");
const LEGACY_ANCHORS: &str = include_str!("../legacy-anchors.json");
const EXPECTATIONS: &str = include_str!("../expectations.json");

/// 矩阵中的一个版本点：一个旧锚点或当前源码 `head`。
#[derive(Clone, Debug)]
pub(crate) struct Point {
    pub(crate) id: String,
    pub(crate) engine_rev: Option<String>,
    /// 传给 Engine 的应用版本：锚点取其首个 Desktop 发布版本，`head` 取最后发布版本的下一个预发布号。
    pub(crate) app_version: String,
}

/// 早于 Engine 的 Desktop 发布：只能以静态资料快照参与，由当前源码打开。
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LegacyAnchor {
    pub(crate) id: String,
    pub(crate) desktop_release: String,
    /// 相对本 crate 根目录的快照目录，内含 `profile/` 与 `SHA256SUMS`。
    pub(crate) fixture: String,
    pub(crate) device_name: String,
    /// 快照中历史条目的原文，用于升级后逐条核对。
    pub(crate) history: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Dimension {
    Upgrade,
    SequentialUpgrade,
    MixedVersions,
    Downgrade,
}

impl Dimension {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Upgrade => "d1",
            Self::SequentialUpgrade => "d2",
            Self::MixedVersions => "d3",
            Self::Downgrade => "d4",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Inviter {
    Old,
    New,
}

/// 升级过程中注入的外部干扰。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Interruption {
    /// 首次升级期间外部持有 Space 转换激活租约，使单设备重建在提交边界失败；随后释放并重启。
    CommitInterrupted,
}

#[derive(Clone, Debug)]
pub(crate) enum Versions {
    Pair { from: Point, to: Point },
    Chain(Vec<Point>),
    Legacy { from: LegacyAnchor, to: Point },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "expected", rename_all = "kebab-case")]
pub(crate) enum Expected {
    Pass {
        /// 完整链中已登记为无法运行的版本点。
        #[serde(default)]
        excluded_points: Vec<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    KnownIncompatible {
        stage: String,
        condition: String,
        reason: String,
        link: String,
    },
    /// 加入方在 `stage` 阶段明确拒绝，且公开拒绝原因为 `rejection_reason`；配对本应失败时使用。
    Rejected {
        stage: String,
        rejection_reason: String,
        reason: String,
        link: String,
    },
    Skip {
        condition: String,
        reason: String,
        link: String,
    },
}

impl Expected {
    /// 非“通过”的登记必须写明原因与链接；排除版本点同样要说明原因。
    fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Pass {
                excluded_points,
                reason,
            } => {
                if !excluded_points.is_empty() && reason.as_deref().is_none_or(str::is_empty) {
                    return Err("excludes points without a reason");
                }
            }
            Self::KnownIncompatible {
                stage,
                condition,
                reason,
                link,
            } => {
                if [stage, condition, reason, link]
                    .iter()
                    .any(|value| value.is_empty())
                {
                    return Err("must record stage, condition, reason and link");
                }
            }
            Self::Rejected {
                stage,
                rejection_reason,
                reason,
                link,
            } => {
                if [stage, rejection_reason, reason, link]
                    .iter()
                    .any(|value| value.is_empty())
                {
                    return Err("must record stage, rejection_reason, reason and link");
                }
            }
            Self::Skip {
                condition,
                reason,
                link,
            } => {
                if [condition, reason, link]
                    .iter()
                    .any(|value| value.is_empty())
                {
                    return Err("must record condition, reason and link");
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CellSpec {
    pub(crate) id: String,
    pub(crate) dimension: Dimension,
    pub(crate) versions: Versions,
    pub(crate) inviter: Option<Inviter>,
    pub(crate) interruption: Option<Interruption>,
    pub(crate) expected: Expected,
    pub(crate) expected_raw: Value,
}

#[derive(Deserialize)]
struct AnchorFile {
    anchors: Vec<AnchorEntry>,
}

#[derive(Deserialize)]
struct AnchorEntry {
    id: String,
    engine_rev: String,
    desktop_releases: Vec<Release>,
}

#[derive(Deserialize)]
struct Release {
    tag: String,
}

#[derive(Deserialize)]
struct LegacyAnchorFile {
    anchors: Vec<LegacyAnchor>,
}

pub(crate) fn legacy_anchors() -> Result<Vec<LegacyAnchor>, String> {
    serde_json::from_str::<LegacyAnchorFile>(LEGACY_ANCHORS)
        .map(|file| file.anchors)
        .map_err(|_| "legacy-anchors.json is invalid".to_owned())
}

pub(crate) fn points() -> Result<Vec<Point>, String> {
    let file: AnchorFile =
        serde_json::from_str(ANCHORS).map_err(|_| "anchors.json is invalid".to_owned())?;
    let mut points = Vec::with_capacity(file.anchors.len() + 1);
    for anchor in &file.anchors {
        let first = anchor
            .desktop_releases
            .first()
            .ok_or_else(|| format!("{} has no Desktop release", anchor.id))?;
        points.push(Point {
            id: anchor.id.clone(),
            engine_rev: Some(anchor.engine_rev.clone()),
            app_version: first.tag.trim_start_matches('v').to_owned(),
        });
    }
    let last = file
        .anchors
        .last()
        .and_then(|anchor| anchor.desktop_releases.last())
        .ok_or_else(|| "anchors.json has no releases".to_owned())?;
    points.push(Point {
        id: "head".into(),
        engine_rev: None,
        app_version: next_prerelease(last.tag.trim_start_matches('v')),
    });
    Ok(points)
}

fn next_prerelease(version: &str) -> String {
    match version.rsplit_once('.') {
        Some((prefix, number)) if version.contains('-') => match number.parse::<u64>() {
            Ok(number) => format!("{prefix}.{}", number + 1),
            Err(_) => format!("{version}.1"),
        },
        _ => format!("{version}-next.1"),
    }
}

pub(crate) fn cell(id: &str) -> Result<CellSpec, String> {
    let expectations: Value = serde_json::from_str(EXPECTATIONS)
        .map_err(|_| "expectations.json is invalid".to_owned())?;
    let raw = expectations["cells"]
        .as_array()
        .and_then(|cells| cells.iter().find(|cell| cell["cell"] == id))
        .cloned()
        .ok_or_else(|| format!("{id} is not registered"))?;
    let expected: Expected = serde_json::from_value(raw.clone())
        .map_err(|_| format!("{id} has an invalid expectation"))?;
    expected
        .validate()
        .map_err(|problem| format!("{id} {problem}"))?;
    let points = points()?;
    let point = |id: &str| {
        points
            .iter()
            .find(|point| point.id == id)
            .cloned()
            .ok_or_else(|| format!("unknown version point {id}"))
    };
    let parts: Vec<&str> = id.split('-').collect();
    let dimension = match parts.first().copied() {
        Some("d1") => Dimension::Upgrade,
        Some("d2") => Dimension::SequentialUpgrade,
        Some("d3") => Dimension::MixedVersions,
        Some("d4") => Dimension::Downgrade,
        _ => return Err(format!("{id} has an unknown dimension")),
    };
    let legacy = legacy_anchors()?;
    let versions = if parts.get(1) == Some(&"chain") {
        Versions::Chain(points.clone())
    } else if let Some(from) = parts
        .get(1)
        .and_then(|id| legacy.iter().find(|anchor| anchor.id == *id))
    {
        let Some(to) = parts.get(2) else {
            return Err(format!("{id} does not name a target version"));
        };
        Versions::Legacy {
            from: from.clone(),
            to: point(to)?,
        }
    } else {
        let (Some(from), Some(to)) = (parts.get(1), parts.get(2)) else {
            return Err(format!("{id} does not name two versions"));
        };
        Versions::Pair {
            from: point(from)?,
            to: point(to)?,
        }
    };
    let (inviter, interruption) = match (parts.get(3).copied(), parts.get(4).copied()) {
        (Some("old"), Some("inviter")) => (Some(Inviter::Old), None),
        (Some("new"), Some("inviter")) => (Some(Inviter::New), None),
        (Some("commit"), Some("interrupted")) => (None, Some(Interruption::CommitInterrupted)),
        (None, None) => (None, None),
        _ => return Err(format!("{id} has an unknown variant")),
    };
    Ok(CellSpec {
        id: id.to_owned(),
        dimension,
        versions,
        inviter,
        interruption,
        expected,
        expected_raw: raw,
    })
}
