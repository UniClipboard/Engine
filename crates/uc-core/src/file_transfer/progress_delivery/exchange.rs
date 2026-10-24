use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;

/// 同端已等待的终态优先于普通查询；不同对端仍按轮转公平启动。
#[derive(Clone, Copy)]
pub enum ExchangePriority {
    Terminal,
    Normal,
}

/// 同时在途的网络交换全局上限。
pub const EXCHANGE_GLOBAL_ACTIVE: usize = 4;
/// 排队等待的交换全局上限。
pub const EXCHANGE_GLOBAL_QUEUED: usize = 64;
pub const EXCHANGE_PEER_QUEUED: usize = 16;

#[derive(Debug, PartialEq, Eq)]
pub enum ExchangeRefusal<W> {
    /// 该对端等待队列已满；工作原样退回，由调用方合并或放弃。
    PeerQueueFull(W),
    /// 全局等待队列已满。
    GlobalQueueFull(W),
}

/// 对端轮转的有界交换队列：全局在途 4、每端在途 1，等待全局 64、每端 16。
///
/// 一个对端的交换一直不结束，只占用它自己的一个在途额度和队列，不阻塞其他对端；
/// 释放额度必须由真实结束触发，不能因等待超时而提前归还仍在执行的本机工作。
#[derive(Debug)]
pub struct ExchangeQueue<P, W> {
    queues: HashMap<P, VecDeque<W>>,
    ring: VecDeque<P>,
    active: HashSet<P>,
    queued: usize,
}

impl<P, W> Default for ExchangeQueue<P, W> {
    fn default() -> Self {
        Self {
            queues: HashMap::new(),
            ring: VecDeque::new(),
            active: HashSet::new(),
            queued: 0,
        }
    }
}

impl<P: Clone + Eq + Hash, W> ExchangeQueue<P, W> {
    pub fn queued(&self) -> usize {
        self.queued
    }

    pub fn active(&self) -> usize {
        self.active.len()
    }

    pub fn enqueue(
        &mut self,
        peer: P,
        work: W,
        priority: ExchangePriority,
    ) -> Result<(), ExchangeRefusal<W>> {
        if self.queued >= EXCHANGE_GLOBAL_QUEUED {
            return Err(ExchangeRefusal::GlobalQueueFull(work));
        }
        let queue = self.queues.entry(peer.clone()).or_default();
        if queue.len() >= EXCHANGE_PEER_QUEUED {
            return Err(ExchangeRefusal::PeerQueueFull(work));
        }
        if queue.is_empty() {
            self.ring.push_back(peer);
        }
        match priority {
            ExchangePriority::Terminal => queue.push_front(work),
            ExchangePriority::Normal => queue.push_back(work),
        }
        self.queued += 1;
        Ok(())
    }

    /// 取下一个可启动的交换：按对端轮转，跳过已有在途交换的对端。
    pub fn next_ready(&mut self) -> Option<(P, W)> {
        if self.active.len() >= EXCHANGE_GLOBAL_ACTIVE {
            return None;
        }
        for _ in 0..self.ring.len() {
            let peer = self.ring.pop_front()?;
            if self.active.contains(&peer) {
                self.ring.push_back(peer);
                continue;
            }
            let queue = self.queues.get_mut(&peer)?;
            let work = queue.pop_front()?;
            self.queued -= 1;
            if queue.is_empty() {
                self.queues.remove(&peer);
            } else {
                self.ring.push_back(peer.clone());
            }
            self.active.insert(peer.clone());
            return Some((peer, work));
        }
        None
    }

    /// 只移除未启动的候选；活动额度仍待真实完成。
    pub fn remove_queued(&mut self, peer: &P, work: &W) -> bool
    where
        W: PartialEq,
    {
        let Some(queue) = self.queues.get_mut(peer) else {
            return false;
        };
        let Some(index) = queue.iter().position(|candidate| candidate == work) else {
            return false;
        };
        queue.remove(index);
        self.queued -= 1;
        if queue.is_empty() {
            self.queues.remove(peer);
            self.ring.retain(|candidate| candidate != peer);
        }
        true
    }
    /// 该对端的在途交换真实结束后归还额度。
    pub fn complete(&mut self, peer: &P) {
        self.active.remove(peer);
    }
}
