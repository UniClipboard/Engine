//! 离线优先成员移除的领域模型(ADR-015)。
//!
//! 成员移除由不可变、可合并的移除意图表达。每个意图绑定空间沿革、因果成员视图、
//! 发起成员实例和目标成员实例,并由发起者在对应因果视图中的凭据签名。任意合法
//! 意图取并集后应用移除优先规则得到有效成员集合;有效成员集合与意图集合共同
//! 计算稳定收敛摘要,所有设备从相同意图集合得到相同摘要。
//!
//! 本模块只承载纯领域逻辑:实例标识推导、意图内容编码与标识一致性、可接受的
//! 规则校验、合并与收敛摘要计算。签名验证和因果证明的密码学验证由端口
//! (`RemovalIntentVerificationPort`) 负责。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::DeviceId;

use super::ports::RemovalViewSnapshot;

/// 一次成功准入形成的成员身份标识。
///
/// 同一设备被移除后重新加入会获得新的成员实例,旧意图不得命中新实例。
/// 实例标识由设备标识与准入凭据签名公钥共同推导:每次准入生成新的签名密钥,
/// 因此新准入自动产生新实例标识,不需要额外的准入序号状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MemberInstanceId([u8; 32]);

impl MemberInstanceId {
    /// 从设备标识与准入凭据签名公钥推导实例标识。
    pub fn derive(device_id: &str, signature_key: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard-member-instance/v1\0");
        hasher.update(device_id.as_bytes());
        hasher.update((signature_key.len() as u64).to_be_bytes());
        hasher.update(signature_key);
        Self(hasher.finalize().into())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for MemberInstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..8] {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// 移除意图的稳定标识。
///
/// 意图标识由不可变内容推导,内容任何字段被篡改都会导致标识不匹配。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RemovalIntentId([u8; 32]);

impl RemovalIntentId {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for RemovalIntentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..8] {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// 移除意图的不可变内容。
///
/// `space_lineage` 为空间沿革(本空间一致);`view_epoch` 与 `view_members`
/// 构成创建意图时的因果成员视图摘要;`initiator` 与 `target` 必须都是该视图
/// 中的不同成员实例。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalIntentContent {
    pub space_lineage: String,
    pub view_epoch: u64,
    pub view_members: Vec<MemberInstanceId>,
    pub initiator: MemberInstanceId,
    pub target: MemberInstanceId,
}

impl RemovalIntentContent {
    /// 规范字节编码,用于推导意图标识与签名负载。
    ///
    /// 编码是确定性的:`view_members` 必须已排序去重。
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut buffer = Vec::with_capacity(128 + self.view_members.len() * 32);
        buffer.extend_from_slice(b"uniclipboard-removal-intent/v1\0");
        buffer.extend_from_slice(self.space_lineage.as_bytes());
        buffer.push(0);
        buffer.extend_from_slice(&self.view_epoch.to_be_bytes());
        buffer.push(0);
        for member in &self.view_members {
            buffer.extend_from_slice(member.as_bytes());
        }
        buffer.push(0);
        buffer.extend_from_slice(self.initiator.as_bytes());
        buffer.extend_from_slice(self.target.as_bytes());
        buffer
    }

    pub fn intent_id(&self) -> RemovalIntentId {
        RemovalIntentId(Sha256::digest(self.canonical_bytes()).into())
    }

    /// 字段级规则校验。
    ///
    /// 密码学校验(签名、证明)不在此处,由端口完成。
    pub fn validate(&self) -> Result<(), RemovalIntentRejection> {
        if self.space_lineage.is_empty() || self.space_lineage.len() > 128 {
            return Err(RemovalIntentRejection::InvalidSpaceLineage);
        }
        if self.view_members.len() > MAX_VIEW_MEMBERS {
            return Err(RemovalIntentRejection::OversizedView);
        }
        if !view_members_are_sorted(&self.view_members) {
            return Err(RemovalIntentRejection::UnsortedView);
        }
        if self.initiator == self.target {
            return Err(RemovalIntentRejection::SelfTarget);
        }
        if !self.view_members.contains(&self.initiator) {
            return Err(RemovalIntentRejection::InitiatorNotInView);
        }
        if !self.view_members.contains(&self.target) {
            return Err(RemovalIntentRejection::TargetNotInView);
        }
        Ok(())
    }
}

/// 视图成员数量上限,防止证明无界增长。
pub const MAX_VIEW_MEMBERS: usize = 256;

fn view_members_are_sorted(members: &[MemberInstanceId]) -> bool {
    members.windows(2).all(|pair| pair[0] < pair[1])
}

/// 因果证明中的一个公开成员身份。
///
/// 这里只能保存设备标识、成员实例和公开签名密钥。完整 MLS 客户端状态包含本机
/// 私有资料，绝不能作为意图证明发送给其他设备。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalCausalProofMember {
    pub device_id: DeviceId,
    pub instance: MemberInstanceId,
    pub signing_public_key: Vec<u8>,
}

/// 可跨设备传输的公开因果证明。
///
/// 验证者用公开签名密钥检查作者签名，再把这份视图与本机保留的历史检查点比对；
/// 它不包含 MLS 状态、私钥、内容密钥或任何可用于恢复本机安全状态的资料。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalCausalProof {
    pub epoch: u64,
    pub members: Vec<RemovalCausalProofMember>,
}

impl RemovalCausalProof {
    pub fn new(epoch: u64, mut members: Vec<RemovalCausalProofMember>) -> Self {
        members.sort_by_key(|member| member.instance);
        Self { epoch, members }
    }

    pub fn members(&self) -> Vec<MemberInstanceId> {
        self.members.iter().map(|member| member.instance).collect()
    }

    pub fn fingerprint(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard-removal-causal-proof/v1\0");
        hasher.update(self.epoch.to_be_bytes());
        for member in &self.members {
            hasher.update(member.device_id.as_str().as_bytes());
            hasher.update([0]);
            hasher.update(member.instance.as_bytes());
            hasher.update((member.signing_public_key.len() as u64).to_be_bytes());
            hasher.update(&member.signing_public_key);
        }
        hasher.finalize().into()
    }

    pub fn matches_content(&self, content: &RemovalIntentContent) -> bool {
        self.epoch == content.view_epoch && self.members() == content.view_members
    }
}

/// 意图信封:不可变内容 + 稳定标识 + 发起者签名 + 公开因果证明。
///
/// `signature` 由发起者在对应因果视图中的凭据对规范内容字节签名;
/// `causal_proof` 为验证签名所需的公开成员视图；完整 MLS 群组状态不得进入
/// 此字段，由上层负责加密持久化。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedRemovalIntent {
    pub content: RemovalIntentContent,
    pub intent_id: RemovalIntentId,
    pub signature: Vec<u8>,
    pub causal_proof: RemovalCausalProof,
}

impl SignedRemovalIntent {
    /// 构造意图信封并推导稳定标识。
    pub fn new(
        content: RemovalIntentContent,
        signature: Vec<u8>,
        causal_proof: RemovalCausalProof,
    ) -> Self {
        let intent_id = content.intent_id();
        Self {
            content,
            intent_id,
            signature,
            causal_proof,
        }
    }

    /// 内容级校验(不含密码学)。
    pub fn validate_content(&self) -> Result<(), RemovalIntentRejection> {
        self.content.validate()?;
        if self.intent_id != self.content.intent_id() {
            return Err(RemovalIntentRejection::IntentIdMismatch);
        }
        if !self.causal_proof.matches_content(&self.content) {
            return Err(RemovalIntentRejection::InvalidCausalProof);
        }
        Ok(())
    }
}

/// 内容级拒绝原因。密码学失败由验证端口表达。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalIntentRejection {
    InvalidSpaceLineage,
    OversizedView,
    UnsortedView,
    SelfTarget,
    InitiatorNotInView,
    TargetNotInView,
    IntentIdMismatch,
    InvalidCausalProof,
}

impl fmt::Display for RemovalIntentRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpaceLineage => formatter.write_str("invalid space lineage"),
            Self::OversizedView => formatter.write_str("causal view exceeds member limit"),
            Self::UnsortedView => formatter.write_str("causal view members are not sorted"),
            Self::SelfTarget => formatter.write_str("initiator cannot remove its own instance"),
            Self::InitiatorNotInView => formatter.write_str("initiator is not in the causal view"),
            Self::TargetNotInView => formatter.write_str("target is not in the causal view"),
            Self::IntentIdMismatch => formatter.write_str("intent id does not match content"),
            Self::InvalidCausalProof => formatter.write_str("causal proof does not match content"),
        }
    }
}

impl std::error::Error for RemovalIntentRejection {}

/// 对外收敛阶段(规格 015)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RemovalPhase {
    /// 本机意图和本机安全限制已生效,其他有效成员可能尚未收敛。
    Applied,
    /// 已发现并合并远端意图,正在形成或分发统一安全状态。
    Converging,
    /// 所有当前已知的有效保留成员都已确认同一收敛摘要和安全状态。
    Complete,
    /// 因果历史不可验证、恢复资料冲突或有效成员集合为空,不能自动继续。
    RecoveryRequired,
}

impl RemovalPhase {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::RecoveryRequired)
    }
}

/// 恢复资料:执行者从自己的分叉成员集合生成的向前安全状态。
///
/// 目标成员集合与收敛摘要完全匹配才被接受;`commit` 为向前提交,
/// `welcome` 在存在需要重新加入的有效成员时存在,`encrypted_key_catalog`
/// 承载新一轮内容密钥目录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalRecoveryMaterial {
    pub convergence_digest: [u8; 32],
    pub effective_members: Vec<MemberInstanceId>,
    pub epoch: u64,
    pub commit: Vec<u8>,
    pub welcome: Option<Vec<u8>>,
    pub encrypted_key_catalog: Vec<u8>,
}

/// 执行者准备好的恢复资料及仅供本机安装的加密检查点。
///
/// `local_checkpoint` 只能随本机收敛状态保存，绝不能发送给其他成员。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalPreparedRecovery {
    pub material: RemovalRecoveryMaterial,
    pub local_checkpoint: Vec<u8>,
}

/// 保留成员在实际应用一轮恢复资料后签发的完成确认。
///
/// `member` 仍指向本轮意图所使用的旧成员实例；签名则由已经应用后的
/// 当前安全状态验证。这样确认既绑定收敛摘要，也不会把旧实例重新带回当前成员集。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalCompletionReceipt {
    pub member: MemberInstanceId,
    pub convergence_digest: [u8; 32],
    pub signature: Vec<u8>,
}

impl RemovalCompletionReceipt {
    pub fn signing_payload(member: MemberInstanceId, convergence_digest: &[u8; 32]) -> Vec<u8> {
        let mut payload = Vec::with_capacity(96);
        payload.extend_from_slice(b"uniclipboard-removal-complete/v1\0");
        payload.extend_from_slice(convergence_digest);
        payload.extend_from_slice(member.as_bytes());
        payload
    }

    pub fn payload(&self) -> Vec<u8> {
        Self::signing_payload(self.member, &self.convergence_digest)
    }
}

/// 执行者恢复流程的持久化状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalRecoveryPersisted {
    pub convergence_digest: [u8; 32],
    pub effective_members: Vec<MemberInstanceId>,
    /// 已收集的备用 key package(成员实例 → key package)。
    pub key_packages: BTreeMap<MemberInstanceId, Vec<u8>>,
    /// 已生成的恢复资料(分发开始后持久化)。
    pub material: Option<RemovalRecoveryMaterial>,
    /// 执行者本机安装恢复资料所需的私有检查点。
    pub local_checkpoint: Option<Vec<u8>>,
    /// 已确认实际应用的成员实例及其可验证确认(执行者收集完成确认)。
    pub delivery_acks: BTreeMap<MemberInstanceId, RemovalCompletionReceipt>,
    /// 已确认收到完成状态的保留成员实例。执行者在完成后仍保留这份有界进度，
    /// 以便网络中断时继续通知尚未收到完成状态的成员。
    pub completion_deliveries: BTreeMap<MemberInstanceId, i64>,
}

/// 已验证因果视图的持久化检查点。
///
/// 检查点只保存证明的摘要和公开的视图形状，不复制 MLS 私有状态。它把某一份
/// 已由本机当前状态或已验收意图锚定的历史固定下来，使之后的迟到意图可以判断为
/// "已知历史" 或 "签名正确但缺少本机基准历史"。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalCausalCheckpoint {
    pub proof_digest: [u8; 32],
    pub view_epoch: u64,
    pub view_members: Vec<MemberInstanceId>,
}

impl RemovalCausalCheckpoint {
    pub fn from_intent(intent: &SignedRemovalIntent) -> Self {
        Self {
            proof_digest: intent.causal_proof.fingerprint(),
            view_epoch: intent.content.view_epoch,
            view_members: intent.content.view_members.clone(),
        }
    }

    pub fn from_view(view: &RemovalViewSnapshot) -> Self {
        Self {
            proof_digest: view.causal_proof.fingerprint(),
            view_epoch: view.causal_proof.epoch,
            view_members: view.causal_proof.members(),
        }
    }
}

/// 收敛状态的持久化模型(跨重启继续)。
///
/// 意图、因果证明、已知意图集合、传播进度、本机安全限制与恢复执行状态
/// 全部包含在此;上层负责整体加密持久化。保存时必须保证:崩溃恢复后
/// 不会出现"意图存在但本机重新信任目标"的状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalPersistedState {
    pub space_lineage: String,
    pub intents: Vec<SignedRemovalIntent>,
    /// 本机已生效的移除目标(本机停止信任、停止同步)。
    pub locally_removed: BTreeSet<MemberInstanceId>,
    /// 本机已生效的移除目标设备。发送路径只读取此集合，避免依赖已经推进
    /// 过的成员视图重新解析旧成员实例。
    pub locally_removed_devices: BTreeSet<DeviceId>,
    /// 本轮因果成员实例到设备标识的稳定映射。恢复后成员会取得新的签名密钥，
    /// 因此完成确认必须通过这份已验证映射核对原成员实例。
    pub member_devices: BTreeMap<MemberInstanceId, DeviceId>,
    /// 已由上一轮恢复资料替换的旧成员实例。它们保留在历史意图中以供验证，
    /// 但不得重新参与下一轮成员计算。
    pub retired_members: BTreeSet<MemberInstanceId>,
    /// 已由本机或已验收意图锚定的因果历史检查点。必须与意图一同保留到空间
    /// 沿革结束，不能由普通清理删除。
    pub causal_history: Vec<RemovalCausalCheckpoint>,
    /// 传播进度:(收件人, 意图) → 确认时间。
    pub peer_exchanges: BTreeMap<(DeviceId, RemovalIntentId), i64>,
    /// 执行者恢复流程(仅执行者持有)。
    pub recovery: Option<RemovalRecoveryPersisted>,
    /// 本机已应用的收敛摘要(完整事实,用于重启后恢复)。
    pub applied_digest: Option<[u8; 32]>,
    /// 当前已完成轮次的保留成员数量。恢复会替换部分旧成员实例；这个数量记录
    /// 该轮实际完成时的保留成员数，不能从淘汰旧实例后的历史集合反推。
    pub completed_member_count: Option<usize>,
    /// 仅用于使旧邀请失效的单调准入编号。每接纳一条此前未知的移除意图递增。
    /// 它属于同一加密状态，不是第二份成员事实。
    pub admission_generation: u64,
    pub phase: RemovalPhase,
    pub updated_at_ms: i64,
}

impl RemovalPersistedState {
    /// 从已知意图集合重建收敛计算。
    pub fn convergence(&self) -> RemovalConvergence {
        let mut convergence = RemovalConvergence::new();
        for intent in &self.intents {
            convergence.insert(intent);
        }
        convergence
    }

    /// 已知意图集合上的有效成员集合。
    pub fn effective_members(&self) -> BTreeSet<MemberInstanceId> {
        self.convergence()
            .effective_members()
            .difference(&self.retired_members)
            .copied()
            .collect()
    }

    /// 本机实例是否已被本机判定移除(本机已观察到自身被移除)。
    pub fn own_instance_removed(&self, own_instance: &MemberInstanceId) -> bool {
        self.locally_removed.contains(own_instance)
    }

    /// 目标设备是否已被本机限制发送。
    pub fn device_is_locally_removed(&self, device_id: &DeviceId) -> bool {
        self.locally_removed_devices.contains(device_id)
    }

    pub fn remembers_causal_history(&self, intent: &SignedRemovalIntent) -> bool {
        let checkpoint = RemovalCausalCheckpoint::from_intent(intent);
        self.causal_history.iter().any(|known| known == &checkpoint)
    }

    pub fn remember_causal_history(&mut self, checkpoint: RemovalCausalCheckpoint) {
        if !self.causal_history.iter().any(|known| known == &checkpoint) {
            self.causal_history.push(checkpoint);
        }
    }
}

/// 对外状态摘要:一次查询恢复完整当前事实,不要求产品端拼接事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberRemovalSummary {
    pub phase: RemovalPhase,
    pub intent_count: usize,
    pub effective_member_count: usize,
    pub convergence_digest: Option<[u8; 32]>,
    pub updated_at_ms: i64,
}

impl MemberRemovalSummary {
    pub fn new(
        phase: RemovalPhase,
        intent_count: usize,
        effective_member_count: usize,
        convergence_digest: Option<[u8; 32]>,
        updated_at_ms: i64,
    ) -> Self {
        Self {
            phase,
            intent_count,
            effective_member_count,
            convergence_digest,
            updated_at_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IntentFacts {
    initiator: MemberInstanceId,
    target: MemberInstanceId,
    view_members: BTreeSet<MemberInstanceId>,
}

/// 已知合法意图集合上的合并计算(纯函数)。
///
/// - 按稳定意图标识去重;
/// - 移除目标取并集,发起者被另一并发意图移除不影响其既有意图的效力;
/// - 有效成员集合 = 已知因果视图成员并集 - 合法目标并集;
/// - 收敛摘要由空间沿革、视图代次、意图集合与有效成员集合确定性计算。
///
/// 计算结果满足交换律、结合律与幂等性。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemovalConvergence {
    intents: BTreeMap<RemovalIntentId, IntentFacts>,
}

impl RemovalConvergence {
    pub fn new() -> Self {
        Self::default()
    }

    /// 插入一个已验证意图。返回 `true` 表示新意图(此前未知)。
    pub fn insert(&mut self, intent: &SignedRemovalIntent) -> bool {
        let facts = IntentFacts {
            initiator: intent.content.initiator,
            target: intent.content.target,
            view_members: intent.content.view_members.iter().copied().collect(),
        };
        self.intents.insert(intent.intent_id, facts).is_none()
    }

    pub fn len(&self) -> usize {
        self.intents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.intents.is_empty()
    }

    pub fn contains(&self, intent_id: &RemovalIntentId) -> bool {
        self.intents.contains_key(intent_id)
    }

    /// 已知合法意图集合(按稳定标识排序)。
    pub fn intent_ids(&self) -> impl Iterator<Item = &RemovalIntentId> {
        self.intents.keys()
    }

    /// 有效成员集合:已知因果视图成员并集减去合法目标并集。
    pub fn effective_members(&self) -> BTreeSet<MemberInstanceId> {
        let mut retained: BTreeSet<MemberInstanceId> = BTreeSet::new();
        for facts in self.intents.values() {
            retained.extend(facts.view_members.iter().copied());
        }
        for facts in self.intents.values() {
            retained.remove(&facts.target);
        }
        retained
    }

    /// 稳定收敛摘要。
    ///
    /// 相同意图集合在任何设备上都得到相同摘要;消息顺序与重复不影响结果。
    pub fn convergence_digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard-removal-convergence/v1\0");
        let mut epochs = Vec::new();
        for facts in self.intents.values() {
            hasher.update(facts.initiator.as_bytes());
            hasher.update(facts.target.as_bytes());
            for member in &facts.view_members {
                hasher.update(member.as_bytes());
            }
            epochs.push(facts.view_members.len() as u64);
        }
        epochs.sort_unstable();
        for epoch in epochs {
            hasher.update(&epoch.to_be_bytes());
        }
        for member in self.effective_members() {
            hasher.update(member.as_bytes());
        }
        hasher.finalize().into()
    }

    /// 确定规则选出的一次性恢复执行者:有效成员实例中字典序最小者。
    ///
    /// 只依赖持久事实,不依赖在线集合、设备时间或到达顺序;执行者重启后
    /// 从相同意图集合得到相同执行者。
    pub fn executor(&self) -> Option<MemberInstanceId> {
        self.effective_members().into_iter().next()
    }

    /// 计算当前对外摘要。
    pub fn summary(&self, now_ms: i64) -> MemberRemovalSummary {
        let effective = self.effective_members();
        MemberRemovalSummary::new(
            if self.is_empty() {
                RemovalPhase::Applied
            } else {
                RemovalPhase::Converging
            },
            self.len(),
            effective.len(),
            if self.is_empty() {
                None
            } else {
                Some(self.convergence_digest())
            },
            now_ms,
        )
    }
}

#[cfg(test)]
mod tests {
    use rand::seq::SliceRandom;
    use rand::{rngs::StdRng, SeedableRng};

    use super::*;

    fn instance(device: &str, key: u8) -> MemberInstanceId {
        MemberInstanceId::derive(device, &[key; 32])
    }

    fn view(_epoch: u64, members: &[MemberInstanceId]) -> Vec<MemberInstanceId> {
        let mut sorted = members.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        sorted
    }

    fn intent(
        lineage: &str,
        epoch: u64,
        members: &[MemberInstanceId],
        initiator: MemberInstanceId,
        target: MemberInstanceId,
    ) -> SignedRemovalIntent {
        let content = RemovalIntentContent {
            space_lineage: lineage.to_owned(),
            view_epoch: epoch,
            view_members: view(epoch, members),
            initiator,
            target,
        };
        let proof = RemovalCausalProof::new(
            epoch,
            content
                .view_members
                .iter()
                .enumerate()
                .map(|(index, instance)| RemovalCausalProofMember {
                    device_id: DeviceId::new(format!("test-member-{index}")),
                    instance: *instance,
                    signing_public_key: instance.as_bytes().to_vec(),
                })
                .collect(),
        );
        SignedRemovalIntent::new(content, vec![1, 2, 3], proof)
    }

    #[test]
    fn member_instance_differs_across_admissions_of_the_same_device() {
        let first = instance("alice", 1);
        let second = instance("alice", 2);
        assert_ne!(first, second);
        assert_eq!(instance("alice", 1), first);
    }

    #[test]
    fn member_instance_differs_across_devices() {
        assert_ne!(instance("alice", 1), instance("bob", 1));
    }

    #[test]
    fn intent_id_is_derived_from_content_and_immutable() {
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let charlie = instance("charlie", 3);
        let signed = intent("space-a", 4, &[alice, bob, charlie], alice, bob);
        signed.validate_content().unwrap();
        assert_eq!(signed.intent_id, signed.content.intent_id());

        let mut tampered = signed.clone();
        tampered.content.target = charlie;
        assert!(tampered.validate_content().is_err());
    }

    #[test]
    fn view_members_are_deduplicated_and_sorted() {
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let charlie = instance("charlie", 3);
        let content = RemovalIntentContent {
            space_lineage: "space-a".to_owned(),
            view_epoch: 1,
            view_members: view(1, &[charlie, alice, bob, alice]),
            initiator: alice,
            target: bob,
        };
        content.validate().unwrap();
        assert_eq!(content.view_members.len(), 3);
        let mut sorted = vec![alice, bob, charlie];
        sorted.sort_unstable();
        assert_eq!(content.view_members, sorted);
    }

    #[test]
    fn rejects_self_target_and_members_outside_view() {
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let mallory = instance("mallory", 9);

        let self_target = RemovalIntentContent {
            space_lineage: "space-a".to_owned(),
            view_epoch: 1,
            view_members: view(1, &[alice, bob]),
            initiator: alice,
            target: alice,
        };
        assert_eq!(
            self_target.validate(),
            Err(RemovalIntentRejection::SelfTarget)
        );

        let initiator_outside = RemovalIntentContent {
            space_lineage: "space-a".to_owned(),
            view_epoch: 1,
            view_members: view(1, &[alice, bob]),
            initiator: mallory,
            target: bob,
        };
        assert_eq!(
            initiator_outside.validate(),
            Err(RemovalIntentRejection::InitiatorNotInView)
        );

        let target_outside = RemovalIntentContent {
            space_lineage: "space-a".to_owned(),
            view_epoch: 1,
            view_members: view(1, &[alice, bob]),
            initiator: alice,
            target: mallory,
        };
        assert_eq!(
            target_outside.validate(),
            Err(RemovalIntentRejection::TargetNotInView)
        );
    }

    #[test]
    fn chained_offline_removals_keep_only_the_first_author() {
        // O01: A 移除 B、B 移除 C,两个意图都有效,最终只保留 A。
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let charlie = instance("charlie", 3);
        let members = vec![alice, bob, charlie];

        let a_removes_b = intent("space-a", 1, &members, alice, bob);
        let b_removes_c = intent("space-a", 1, &members, bob, charlie);

        let mut convergence = RemovalConvergence::new();
        assert!(convergence.insert(&a_removes_b));
        assert!(convergence.insert(&b_removes_c));

        assert_eq!(convergence.effective_members(), BTreeSet::from([alice]));
        assert_eq!(convergence.executor(), Some(alice));
    }

    #[test]
    fn concurrent_removals_take_the_target_union() {
        // C04: A 移除 B、C 移除 D,最终保留 A、C。
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let charlie = instance("charlie", 3);
        let dave = instance("dave", 4);
        let members = vec![alice, bob, charlie, dave];

        let mut convergence = RemovalConvergence::new();
        convergence.insert(&intent("space-a", 1, &members, alice, bob));
        convergence.insert(&intent("space-a", 1, &members, charlie, dave));

        assert_eq!(
            convergence.effective_members(),
            BTreeSet::from([alice, charlie])
        );
    }

    #[test]
    fn mutual_removal_keeps_only_the_third_member() {
        // C01: A 移除 B、B 移除 A,最终只保留 C。
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let charlie = instance("charlie", 3);
        let members = vec![alice, bob, charlie];

        let mut convergence = RemovalConvergence::new();
        convergence.insert(&intent("space-a", 1, &members, alice, bob));
        convergence.insert(&intent("space-a", 1, &members, bob, alice));

        assert_eq!(convergence.effective_members(), BTreeSet::from([charlie]));
    }

    #[test]
    fn two_offline_members_mutually_removing_each_other_leave_no_effective_member() {
        // C03: 两个成员各自在完全离线时移除对方，合并后不允许任一端自称幸存。
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let members = vec![alice, bob];

        let mut convergence = RemovalConvergence::new();
        convergence.insert(&intent("space-a", 1, &members, alice, bob));
        convergence.insert(&intent("space-a", 1, &members, bob, alice));

        assert!(convergence.effective_members().is_empty());
        assert_eq!(convergence.executor(), None);
    }

    #[test]
    fn everyone_removed_enters_recovery_required() {
        // C02: A 移除 B、B 移除 C、C 移除 A,有效成员为空。
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let charlie = instance("charlie", 3);
        let members = vec![alice, bob, charlie];

        let mut convergence = RemovalConvergence::new();
        convergence.insert(&intent("space-a", 1, &members, alice, bob));
        convergence.insert(&intent("space-a", 1, &members, bob, charlie));
        convergence.insert(&intent("space-a", 1, &members, charlie, alice));

        assert!(convergence.effective_members().is_empty());
        assert_eq!(convergence.executor(), None);
    }

    #[test]
    fn duplicate_and_overlapping_targets_are_idempotent() {
        // C06: A、B 各自移除 C,C 同时移除 D,最终保留 A、B。
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let charlie = instance("charlie", 3);
        let dave = instance("dave", 4);
        let members = vec![alice, bob, charlie, dave];

        let mut convergence = RemovalConvergence::new();
        let a_removes_c = intent("space-a", 1, &members, alice, charlie);
        let b_removes_c = intent("space-a", 1, &members, bob, charlie);
        let c_removes_d = intent("space-a", 1, &members, charlie, dave);

        assert!(convergence.insert(&a_removes_c));
        assert!(convergence.insert(&b_removes_c));
        assert!(convergence.insert(&c_removes_d));
        let first_digest = convergence.convergence_digest();
        let first_effective = convergence.effective_members();

        // 幂等:重复插入同一意图不改变结果。
        assert!(!convergence.insert(&a_removes_c));
        assert!(!convergence.insert(&c_removes_d));
        assert_eq!(convergence.effective_members(), first_effective);
        assert_eq!(convergence.convergence_digest(), first_digest);

        assert_eq!(
            convergence.effective_members(),
            BTreeSet::from([alice, bob])
        );
    }

    #[test]
    fn merge_is_commutative_and_associative() {
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let charlie = instance("charlie", 3);
        let dave = instance("dave", 4);
        let members = vec![alice, bob, charlie, dave];

        let intents = vec![
            intent("space-a", 1, &members, alice, bob),
            intent("space-a", 1, &members, charlie, dave),
        ];

        let mut order_a = RemovalConvergence::new();
        order_a.insert(&intents[0]);
        order_a.insert(&intents[1]);

        let mut order_b = RemovalConvergence::new();
        order_b.insert(&intents[1]);
        order_b.insert(&intents[0]);

        // 结合:分成两个集合分别合并再合并。
        let mut partial = RemovalConvergence::new();
        partial.insert(&intents[0]);
        let mut combined = RemovalConvergence::new();
        for id in partial.intent_ids() {
            combined.insert(
                &intents
                    .iter()
                    .find(|intent| &intent.intent_id == id)
                    .unwrap(),
            );
        }
        combined.insert(&intents[1]);

        assert_eq!(order_a.effective_members(), order_b.effective_members());
        assert_eq!(order_a.convergence_digest(), order_b.convergence_digest());
        assert_eq!(combined.convergence_digest(), order_a.convergence_digest());
    }

    #[test]
    fn randomized_duplicate_delivery_keeps_the_same_convergence() {
        // C08: 对同一组意图反复改变投递顺序并插入重复，结果必须完全相同。
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let charlie = instance("charlie", 3);
        let dave = instance("dave", 4);
        let erin = instance("erin", 5);
        let members = vec![alice, bob, charlie, dave, erin];
        let intents = vec![
            intent("space-a", 1, &members, alice, bob),
            intent("space-a", 1, &members, bob, charlie),
            intent("space-a", 1, &members, charlie, dave),
            intent("space-a", 1, &members, dave, erin),
        ];
        let mut expected = RemovalConvergence::new();
        for intent in &intents {
            expected.insert(intent);
        }
        let expected_members = expected.effective_members();
        let expected_digest = expected.convergence_digest();

        for seed in 0..128_u64 {
            let mut order = vec![0, 1, 2, 3, 0, 2, 1, 3, 3, 0];
            order.shuffle(&mut StdRng::seed_from_u64(seed));
            let mut convergence = RemovalConvergence::new();
            for index in order {
                convergence.insert(&intents[index]);
            }
            assert_eq!(convergence.effective_members(), expected_members);
            assert_eq!(convergence.convergence_digest(), expected_digest);
        }
    }

    #[test]
    fn digest_is_stable_across_duplicate_delivery() {
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let charlie = instance("charlie", 3);
        let members = vec![alice, bob, charlie];

        let a_removes_b = intent("space-a", 1, &members, alice, bob);

        let mut convergence = RemovalConvergence::new();
        convergence.insert(&a_removes_b);
        let digest = convergence.convergence_digest();

        for _ in 0..5 {
            convergence.insert(&a_removes_b);
        }
        assert_eq!(convergence.convergence_digest(), digest);
        assert_eq!(convergence.len(), 1);
    }

    #[test]
    fn causal_successor_removal_advances_the_digest() {
        // C10: A、C 观察到 B 被移除后,A 从新视图移除 C。
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let charlie = instance("charlie", 3);
        let members = vec![alice, bob, charlie];

        let mut convergence = RemovalConvergence::new();
        let a_removes_b = intent("space-a", 1, &members, alice, bob);
        convergence.insert(&a_removes_b);
        let first_digest = convergence.convergence_digest();

        let new_view = vec![alice, charlie];
        let a_removes_c = intent("space-a", 2, &new_view, alice, charlie);
        assert!(convergence.insert(&a_removes_c));

        assert_eq!(convergence.effective_members(), BTreeSet::from([alice]));
        assert_ne!(convergence.convergence_digest(), first_digest);
    }

    #[test]
    fn executor_is_deterministic_and_survives_reconstruction() {
        let alice = instance("alice", 1);
        let bob = instance("bob", 2);
        let charlie = instance("charlie", 3);
        let members = vec![alice, bob, charlie];

        let mut convergence = RemovalConvergence::new();
        convergence.insert(&intent("space-a", 1, &members, alice, bob));
        let first_executor = convergence.executor();

        // 用不同插入顺序重建,执行者不变。
        let mut rebuilt = RemovalConvergence::new();
        rebuilt.insert(&intent("space-a", 1, &members, alice, bob));
        assert_eq!(rebuilt.executor(), first_executor);

        // 执行者是有效成员。
        assert!(convergence
            .effective_members()
            .contains(&first_executor.unwrap()));
    }
}
