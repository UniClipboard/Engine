//! 发送侧投影按原意图接受顺序有界保留；消息更新和重连不改变顺序。
use std::collections::VecDeque;

pub const PROJECTION_GLOBAL_CAPACITY: usize = 256;
pub const PROJECTION_PEER_CAPACITY: usize = 64;

pub struct ProgressProjectionCapacity<P, K> {
    retained: VecDeque<(P, K)>,
    closed: bool,
}
impl<P, K> Default for ProgressProjectionCapacity<P, K> {
    fn default() -> Self {
        Self {
            retained: VecDeque::new(),
            closed: false,
        }
    }
}
pub enum ProjectionCapacityInput<P, K> {
    Retain { peer: P, key: K },
    Remove(K),
    Clear,
    Close,
}
#[derive(Debug, PartialEq, Eq)]
pub enum ProjectionRetention<K> {
    Unchanged,
    Closed,
    /// 发布删除/会话封口前退休这些投影；实际文件动作仍由原负责人处理。
    Retired(Vec<K>),
    /// 新记录发布前先退休被淘汰投影；不取消实际文件动作。
    Inserted {
        evicted: Option<K>,
    },
}
impl<P: Eq, K: Eq + Clone> ProgressProjectionCapacity<P, K> {
    pub fn is_closed(&self) -> bool {
        self.closed
    }
    pub fn apply(&mut self, input: ProjectionCapacityInput<P, K>) -> ProjectionRetention<K> {
        if self.closed {
            return ProjectionRetention::Closed;
        }
        let (peer, key) = match input {
            ProjectionCapacityInput::Retain { peer, key } => (peer, key),
            ProjectionCapacityInput::Remove(key) => {
                let before = self.retained.len();
                self.retained.retain(|(_, old)| *old != key);
                return if before != self.retained.len() {
                    ProjectionRetention::Retired(vec![key])
                } else {
                    ProjectionRetention::Unchanged
                };
            }
            ProjectionCapacityInput::Clear | ProjectionCapacityInput::Close => {
                self.closed = matches!(input, ProjectionCapacityInput::Close);
                return ProjectionRetention::Retired(
                    self.retained.drain(..).map(|(_, key)| key).collect(),
                );
            }
        };
        if self.retained.iter().any(|(_, old)| *old == key) {
            return ProjectionRetention::Unchanged;
        }
        let evicted = if self
            .retained
            .iter()
            .filter(|(owner, _)| *owner == peer)
            .count()
            >= PROJECTION_PEER_CAPACITY
        {
            self.retained
                .iter()
                .position(|(owner, _)| *owner == peer)
                .and_then(|index| self.retained.remove(index))
                .map(|(_, key)| key)
        } else if self.retained.len() >= PROJECTION_GLOBAL_CAPACITY {
            self.retained.pop_front().map(|(_, key)| key)
        } else {
            None
        };
        self.retained.push_back((peer, key));
        ProjectionRetention::Inserted { evicted }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn retain(
        state: &mut ProgressProjectionCapacity<u16, (u16, u16)>,
        peer: u16,
        n: u16,
    ) -> ProjectionRetention<(u16, u16)> {
        state.apply(ProjectionCapacityInput::Retain {
            peer,
            key: (peer, n),
        })
    }
    #[test]
    fn peer_pressure_retires_only_that_peers_oldest_without_renewing_duplicates() {
        let mut state = ProgressProjectionCapacity::default();
        retain(&mut state, 2, 0);
        for n in 0..64 {
            retain(&mut state, 1, n);
        }
        assert_eq!(retain(&mut state, 1, 0), ProjectionRetention::Unchanged);
        assert_eq!(
            retain(&mut state, 1, 64),
            ProjectionRetention::Inserted {
                evicted: Some((1, 0))
            }
        );
        assert_eq!(retain(&mut state, 2, 0), ProjectionRetention::Unchanged);
    }
    #[test]
    fn global_pressure_retires_oldest_and_removal_does_not_consume_capacity() {
        let mut state = ProgressProjectionCapacity::default();
        for p in 0..4 {
            for n in 0..64 {
                retain(&mut state, p, n);
            }
        }
        assert_eq!(
            retain(&mut state, 4, 0),
            ProjectionRetention::Inserted {
                evicted: Some((0, 0))
            }
        );
        assert_eq!(
            state.apply(ProjectionCapacityInput::Remove((0, 1))),
            ProjectionRetention::Retired(vec![(0, 1)])
        );
        assert_eq!(
            retain(&mut state, 4, 1),
            ProjectionRetention::Inserted { evicted: None }
        );
        assert_eq!(state.retained.len(), 256);
        state.apply(ProjectionCapacityInput::Clear);
        assert_eq!(
            retain(&mut state, 0, 0),
            ProjectionRetention::Inserted { evicted: None }
        );
        state.apply(ProjectionCapacityInput::Close);
        assert_eq!(retain(&mut state, 0, 1), ProjectionRetention::Closed);
    }
}
