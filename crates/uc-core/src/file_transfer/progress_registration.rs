//! 单个已授权接收设备／源条目的登记围栏；不比较随机标识或跨进程修订。
use self::{
    RegistrationEffect as Effect, RegistrationInput as Input, RegistrationRefusal as Refusal,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RegistrationDescriptor<I> {
    pub producer: I,
    pub revision: u64,
    pub source: I,
    pub attempt: I,
    pub item_count: u16,
}

/// 生命周期属于一个 Space 世代、认证设备和源条目；不得跨这些边界复用。
/// 连接世代由完整负责人单调分配，挑战与 binding 每次重新生成且不得复用。
/// 旧连接上的帧只能携带其原世代；不能从消息正文提供或重写世代。
pub struct ProgressRegistration<I> {
    source: I,
    generation: u64,
    pending: Option<I>,
    producer_on_connection: Option<I>,
    descriptor: Option<RegistrationDescriptor<I>>,
    binding: Option<I>,
}

pub enum RegistrationInput<I> {
    ReplaceConnection {
        generation: u64,
    },
    Challenge {
        generation: u64,
        nonce: I,
    },
    Describe {
        generation: u64,
        nonce: I,
        binding: I,
        descriptor: RegistrationDescriptor<I>,
    },
    Notification {
        generation: u64,
        binding: I,
        source: I,
        attempt: I,
        revision: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationRefusal {
    RetiredConnection,
    StaleChallenge,
    WrongSource,
    EmptyPlan,
    ProducerChangedOnConnection,
    RevisionRegressed,
    RevisionConflict,
    WrongBinding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationEffect {
    /// 保存新世代后，停止旧连接交换；保留既知描述用于修订判定。
    ConnectionReplaced,
    /// 保存挑战后才可发出；同世代先前挑战不再能完成登记。
    SendChallenge,
    /// 保存新绑定后才可响应 Bound；投影采用与响应顺序由完整负责人保证。
    SendBound,
    /// 只表示身份匹配；完整负责人仍须校验逻辑项、轮次、序号与结果。
    CheckNotification,
    /// 不替换投影、不刷新联系期限，也不释放其他任务资源。
    Refused(RegistrationRefusal),
}

impl<I: Copy + Eq> ProgressRegistration<I> {
    pub fn new(source: I) -> Self {
        Self {
            source,
            generation: 0,
            pending: None,
            producer_on_connection: None,
            descriptor: None,
            binding: None,
        }
    }

    pub fn apply(&mut self, input: RegistrationInput<I>) -> RegistrationEffect {
        match input {
            Input::ReplaceConnection { generation } => {
                if generation <= self.generation {
                    return Effect::Refused(Refusal::RetiredConnection);
                }
                self.generation = generation;
                self.pending = None;
                self.producer_on_connection = None;
                self.binding = None;
                Effect::ConnectionReplaced
            }
            Input::Challenge { generation, nonce } => {
                if generation != self.generation || generation == 0 {
                    return Effect::Refused(Refusal::RetiredConnection);
                }
                self.pending = Some(nonce);
                Effect::SendChallenge
            }
            Input::Describe {
                generation,
                nonce,
                binding,
                descriptor,
            } => {
                if generation != self.generation || generation == 0 {
                    return Effect::Refused(Refusal::RetiredConnection);
                }
                if self.pending != Some(nonce) {
                    return Effect::Refused(Refusal::StaleChallenge);
                }
                // 当前挑战最多完成一次；拒收后必须重新挑战。
                self.pending = None;
                if descriptor.source != self.source {
                    return Effect::Refused(Refusal::WrongSource);
                }
                if descriptor.item_count == 0 {
                    return Effect::Refused(Refusal::EmptyPlan);
                }
                if self
                    .producer_on_connection
                    .is_some_and(|producer| producer != descriptor.producer)
                {
                    return Effect::Refused(Refusal::ProducerChangedOnConnection);
                }
                if let Some(previous) = self
                    .descriptor
                    .filter(|previous| previous.producer == descriptor.producer)
                {
                    if descriptor.revision < previous.revision {
                        return Effect::Refused(Refusal::RevisionRegressed);
                    }
                    if descriptor.revision == previous.revision && descriptor != previous {
                        return Effect::Refused(Refusal::RevisionConflict);
                    }
                }
                self.producer_on_connection = Some(descriptor.producer);
                self.descriptor = Some(descriptor);
                self.binding = Some(binding);
                Effect::SendBound
            }
            Input::Notification {
                generation,
                binding,
                source,
                attempt,
                revision,
            } => {
                if generation != self.generation || generation == 0 {
                    return Effect::Refused(Refusal::RetiredConnection);
                }
                if self.binding == Some(binding)
                    && self.descriptor.is_some_and(|descriptor| {
                        descriptor.source == source
                            && descriptor.attempt == attempt
                            && descriptor.revision == revision
                    })
                {
                    Effect::CheckNotification
                } else {
                    Effect::Refused(Refusal::WrongBinding)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
