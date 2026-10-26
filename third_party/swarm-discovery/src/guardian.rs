// UniClipboard 修改：接口接收者具有显式生命周期，期望接口集合按有界退避重试安装。
// 详见 UNICLIPBOARD_PATCH.md。
use crate::{
    receiver::{receiver, ReceiverError},
    sender::{self, sender},
    socket::Sockets,
    updater::updater,
    Discoverer,
};
use acto::{
    AcTokioRuntime, ActoCell, ActoHandle, ActoInput, ActoRef, ActoRuntime, PanicOrAbort,
    TokioJoinHandle,
};
use hickory_proto::rr::Name;
use std::{
    collections::{BTreeSet, HashMap},
    future::{pending, poll_fn, Future},
    mem::replace,
    net::{IpAddr, Ipv4Addr},
    pin::Pin,
    task::Poll,
    time::Duration,
};
use tokio::time::{sleep_until, Instant};

/// 接口安装失败后的首次重试延迟。
const INTERFACE_RETRY_INITIAL: Duration = Duration::from_secs(1);
/// 接口安装重试的最大延迟；持续失败的接口最多按此间隔重试。
const INTERFACE_RETRY_MAX: Duration = Duration::from_secs(60);

pub enum Input {
    RemoveAll,
    RemovePort(u16),
    RemoveAddr(IpAddr),
    AddAddr(u16, Vec<IpAddr>),
    SetTxt(String, Option<String>),
    RemoveTxt(String),
    AddInterface(IpAddr),
    RemoveInterface(IpAddr),
    SetInterfaces(BTreeSet<Ipv4Addr>),
}

type ReceiverResult = Result<(), ReceiverError>;
type GuardianCell = ActoCell<Input, AcTokioRuntime, ReceiverResult>;

/// 单个接口的接收 actor：引用决定其存续，句柄用于等待其真正退出。
///
/// 接口接收者不交给 acto 监督：监督句柄由 `ActoCell` 持有，移除接口时无法等待它结束，
/// 而任何受监督 actor 结束都会让 guardian 停止整个发现服务。
struct InterfaceReceiver {
    stop: ActoRef<()>,
    handle: TokioJoinHandle<ReceiverResult>,
}

impl InterfaceReceiver {
    /// 请求停止并等待任务结束；返回后接收者持有的套接字引用已经释放。
    async fn stop(self) {
        let Self { stop, handle } = self;
        drop(stop);
        let _ = handle.join().await;
    }
}

/// 接口所有者：唯一维护期望接口集合、已安装接口及其接收者，并负责安装重试。
///
/// 只有收到 [`Input::SetInterfaces`] 后才进入期望集合模式并重试失败的安装；
/// 只使用构建器初始接口或 `add_interface_v4` 的调用方保持上游的单次尝试行为。
struct Interfaces {
    rt: AcTokioRuntime,
    sockets: Sockets,
    service_name: Name,
    sender: ActoRef<sender::MdnsMsg>,
    receivers: HashMap<Ipv4Addr, InterfaceReceiver>,
    desired: Option<BTreeSet<Ipv4Addr>>,
    retry_at: Option<Instant>,
    retry_delay: Duration,
}

impl Interfaces {
    /// 为已有套接字的接口启动接收者；没有套接字时返回 `false`。
    fn start_receiver(&mut self, addr: Ipv4Addr) -> bool {
        if self.receivers.contains_key(&addr) {
            return true;
        }
        let Some(socket) = self.sockets.get_interface_socket_v4(addr) else {
            return false;
        };
        let service_name = self.service_name.clone();
        let target = self.sender.clone();
        let spawned = self
            .rt
            .spawn_actor(&format!("receiver_interface_{addr}"), move |ctx| {
                receiver(ctx, service_name, socket, target)
            });
        self.receivers.insert(
            addr,
            InterfaceReceiver {
                stop: spawned.me,
                handle: spawned.handle,
            },
        );
        true
    }

    /// 只有套接字和接收者都已就绪才算安装成功。
    fn install(&mut self, addr: Ipv4Addr) -> bool {
        if self.receivers.contains_key(&addr) {
            return true;
        }
        if let Err(e) = self.sockets.add_interface_v4(addr) {
            tracing::debug!("interface addition failed: {}", e);
            return false;
        }
        self.start_receiver(addr)
    }

    /// 先停止并等待接收者退出，再释放接口套接字。
    async fn retire(&mut self, addr: Ipv4Addr) {
        if let Some(receiver) = self.receivers.remove(&addr) {
            receiver.stop().await;
        }
        self.sockets.remove_interface_v4(addr);
    }

    async fn add(&mut self, addr: Ipv4Addr) {
        match &mut self.desired {
            Some(desired) => {
                desired.insert(addr);
                self.reconcile().await;
            }
            None => {
                self.install(addr);
            }
        }
    }

    async fn remove(&mut self, addr: Ipv4Addr) {
        match &mut self.desired {
            Some(desired) => {
                desired.remove(&addr);
                self.reconcile().await;
            }
            None => self.retire(addr).await,
        }
    }

    async fn set(&mut self, desired: BTreeSet<Ipv4Addr>) {
        self.desired = Some(desired);
        self.reconcile().await;
    }

    /// 让已安装接口与期望集合一致；仍有接口未安装时按有界退避安排下一次尝试。
    async fn reconcile(&mut self) {
        let Some(desired) = self.desired.clone() else {
            return;
        };
        let stale: Vec<Ipv4Addr> = self
            .receivers
            .keys()
            .filter(|addr| !desired.contains(addr))
            .copied()
            .collect();
        for addr in stale {
            self.retire(addr).await;
        }
        let mut complete = true;
        for addr in desired {
            complete &= self.install(addr);
        }
        if complete {
            self.retry_at = None;
            self.retry_delay = INTERFACE_RETRY_INITIAL;
        } else {
            tracing::debug!(
                delay = ?self.retry_delay,
                "interface installation incomplete; retry scheduled"
            );
            self.retry_at = Some(Instant::now() + self.retry_delay);
            self.retry_delay = (self.retry_delay * 2).min(INTERFACE_RETRY_MAX);
        }
    }

    /// 等待任一接口接收者自行结束；正常移除的接收者已先从表中删除，不会出现在这里。
    fn next_exit(&mut self) -> impl Future<Output = Result<ReceiverResult, PanicOrAbort>> + '_ {
        poll_fn(|cx| {
            for receiver in self.receivers.values_mut() {
                if let Poll::Ready(result) = ActoHandle::poll(Pin::new(&mut receiver.handle), cx) {
                    return Poll::Ready(result);
                }
            }
            Poll::Pending
        })
    }
}

async fn retry_due(at: Option<Instant>) {
    match at {
        Some(at) => sleep_until(at).await,
        None => pending().await,
    }
}

enum Event {
    Input(ActoInput<Input, ReceiverResult>),
    InterfaceExit(Result<ReceiverResult, PanicOrAbort>),
    Retry,
}

pub async fn guardian(
    mut ctx: GuardianCell,
    mut discoverer: Discoverer,
    sockets: Sockets,
    service_name: Name,
) {
    let callback = replace(&mut discoverer.callback, Box::new(|_, _| {}));
    let tau = discoverer.tau;
    let phi = discoverer.phi;
    let upd_ref = ctx.supervise(
        ctx.spawn("updater", move |ctx| updater(ctx, tau, phi, callback))
            .map_handle(Ok),
    );

    let sockets2 = sockets.clone();
    let sn = service_name.clone();
    let snd_ref = ctx.supervise(
        ctx.spawn("sender", move |ctx| {
            sender(ctx, sockets, upd_ref, discoverer, sn)
        })
        .map_handle(Ok),
    );

    // 主套接字接收者与发现服务同寿命；保留引用，因为接收者把引用全部释放视为停止请求。
    let mut main_receivers = Vec::new();
    if let Some(v4) = sockets2.v4() {
        let service_name = service_name.clone();
        let snd_ref = snd_ref.clone();
        main_receivers.push(ctx.spawn_supervised("receiver_v4", move |ctx| {
            receiver(ctx, service_name, v4, snd_ref)
        }));
    }

    if let Some(v6) = sockets2.v6() {
        let service_name = service_name.clone();
        let snd_ref = snd_ref.clone();
        main_receivers.push(ctx.spawn_supervised("receiver_v6", move |ctx| {
            receiver(ctx, service_name, v6, snd_ref)
        }));
    }

    let mut interfaces = Interfaces {
        rt: ctx.rt().clone(),
        sockets: sockets2,
        service_name,
        sender: snd_ref.clone(),
        receivers: HashMap::new(),
        desired: None,
        retry_at: None,
        retry_delay: INTERFACE_RETRY_INITIAL,
    };

    // Start receivers for initial interface sockets
    for addr in interfaces.sockets.get_all_interface_addresses_v4() {
        if interfaces.start_receiver(addr) {
            tracing::info!("Started receiver for initial interface {}", addr);
        }
    }

    // 受监督 actor 或接口接收者意外结束时停止；正常移除接口不会触发停止。
    loop {
        let retry_at = interfaces.retry_at;
        let event = tokio::select! {
            input = ctx.recv() => Event::Input(input),
            result = interfaces.next_exit() => Event::InterfaceExit(result),
            () = retry_due(retry_at) => Event::Retry,
        };
        let msg = match event {
            Event::Input(ActoInput::NoMoreSenders) => continue,
            Event::Input(ActoInput::Supervision { id, name, result }) => {
                match result {
                    Ok(Ok(_)) => tracing::warn!("actor {:?} ({}) stopped", id, name),
                    Ok(Err(e)) => {
                        tracing::warn!("actor {:?} ({}) failed: {}", id, name, e)
                    }
                    Err(e) => {
                        tracing::warn!("actor {:?} ({}) aborted: {}", id, name, e);
                    }
                }
                break;
            }
            // 接口接收者意外结束与上游受监督接收者一致：停止整个发现服务。
            Event::InterfaceExit(result) => {
                match result {
                    Ok(Ok(())) => tracing::warn!("interface receiver stopped"),
                    Ok(Err(e)) => tracing::warn!("interface receiver failed: {}", e),
                    Err(e) => tracing::warn!("interface receiver aborted: {}", e),
                }
                break;
            }
            Event::Retry => {
                interfaces.reconcile().await;
                continue;
            }
            Event::Input(ActoInput::Message(msg)) => msg,
        };
        match msg {
            Input::AddInterface(IpAddr::V4(addr)) => interfaces.add(addr).await,
            Input::RemoveInterface(IpAddr::V4(addr)) => interfaces.remove(addr).await,
            Input::AddInterface(IpAddr::V6(_)) | Input::RemoveInterface(IpAddr::V6(_)) => {}
            Input::SetInterfaces(desired) => interfaces.set(desired).await,
            msg => {
                snd_ref.send(sender::MdnsMsg::Update(msg));
            }
        }
    }
    drop(main_receivers);
}
