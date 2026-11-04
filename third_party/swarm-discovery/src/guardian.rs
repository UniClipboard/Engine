// UniClipboard 修改：接口接收者具有显式生命周期，IPv4 与 IPv6 期望接口集合按有界退避重试安装。
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
    collections::{BTreeMap, BTreeSet, HashMap},
    future::{pending, poll_fn, Future},
    mem::replace,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    pin::Pin,
    sync::Arc,
    task::Poll,
    time::Duration,
};
use tokio::{
    net::UdpSocket,
    time::{sleep_until, Instant},
};

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
    /// IPv6 期望接口集合：接口索引到发送套接字绑定的本机地址。
    SetInterfacesV6(BTreeMap<u32, Ipv6Addr>),
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
/// 只有收到 [`Input::SetInterfaces`] 或 [`Input::SetInterfacesV6`] 后，对应协议族才进入
/// 期望集合模式并重试失败的安装；只使用构建器初始接口或 `add_interface_v4` 的调用方保持
/// 上游的单次尝试行为。IPv6 接口按操作系统接口索引维护，两个协议族共用同一退避节奏。
struct Interfaces {
    rt: AcTokioRuntime,
    sockets: Sockets,
    service_name: Name,
    sender: ActoRef<sender::MdnsMsg>,
    receivers: HashMap<Ipv4Addr, InterfaceReceiver>,
    desired: Option<BTreeSet<Ipv4Addr>>,
    receivers_v6: HashMap<u32, InterfaceReceiver>,
    desired_v6: Option<BTreeMap<u32, Ipv6Addr>>,
    retry_at: Option<Instant>,
    retry_delay: Duration,
}

impl Interfaces {
    fn spawn_receiver(&self, name: &str, socket: Arc<UdpSocket>) -> InterfaceReceiver {
        let service_name = self.service_name.clone();
        let target = self.sender.clone();
        let spawned = self
            .rt
            .spawn_actor(name, move |ctx| receiver(ctx, service_name, socket, target));
        InterfaceReceiver {
            stop: spawned.me,
            handle: spawned.handle,
        }
    }

    /// 为已有套接字的接口启动接收者；没有套接字时返回 `false`。
    fn start_receiver(&mut self, addr: Ipv4Addr) -> bool {
        if self.receivers.contains_key(&addr) {
            return true;
        }
        let Some(socket) = self.sockets.get_interface_socket_v4(addr) else {
            return false;
        };
        let receiver = self.spawn_receiver(&format!("receiver_interface_{addr}"), socket);
        self.receivers.insert(addr, receiver);
        true
    }

    /// 为已有套接字的 IPv6 接口启动接收者；没有套接字时返回 `false`。
    fn start_receiver_v6(&mut self, index: u32) -> bool {
        if self.receivers_v6.contains_key(&index) {
            return true;
        }
        let Some(socket) = self.sockets.get_interface_socket_v6(index) else {
            return false;
        };
        let receiver = self.spawn_receiver("receiver_interface_v6", socket);
        self.receivers_v6.insert(index, receiver);
        true
    }

    /// 只有套接字、接收者以及主通配套接字上的组播成员关系都已就绪才算安装成功。
    ///
    /// 已安装的地址也要重新确认成员关系：移除同一网卡上的另一地址会让该网卡离开组播组，
    /// 重新加入失败时这里返回 `false`，期望集合模式据此按退避重试。
    fn install(&mut self, addr: Ipv4Addr) -> bool {
        if let Err(e) = self.sockets.add_interface_v4(addr) {
            tracing::debug!("interface addition failed: {}", e);
            return false;
        }
        if !self.start_receiver(addr) {
            return false;
        }
        match self.sockets.ensure_group_on_main_v4(addr) {
            Ok(()) => true,
            Err(e) => {
                tracing::debug!("interface multicast membership incomplete: {}", e);
                false
            }
        }
    }

    /// 先停止并等待接收者退出，再释放接口套接字。
    async fn retire(&mut self, addr: Ipv4Addr) {
        if let Some(receiver) = self.receivers.remove(&addr) {
            receiver.stop().await;
        }
        self.sockets.remove_interface_v4(addr);
    }

    /// 与 [`Self::install`] 相同的完整性要求：IPv6 接口的套接字、接收者以及主通配 IPv6
    /// 套接字上该接口索引的组播成员关系都就绪才算安装成功。
    ///
    /// 已安装接口的绑定地址与期望不同时（例如原地址已被移除）先完整移除再重新安装。
    /// 没有主通配 IPv6 套接字（IPv6 不可用或只启用 IPv4）时无从接收，不安装也不重试。
    async fn install_v6(&mut self, index: u32, addr: Ipv6Addr) -> bool {
        if !self.sockets.has_v6() {
            return true;
        }
        if self
            .sockets
            .interface_addr_v6(index)
            .is_some_and(|bound| bound != addr)
        {
            self.retire_v6(index).await;
        }
        if let Err(e) = self.sockets.add_interface_v6(index, addr) {
            tracing::debug!("IPv6 interface addition failed: {}", e);
            return false;
        }
        if !self.start_receiver_v6(index) {
            return false;
        }
        match self.sockets.ensure_group_on_main_v6(index) {
            Ok(()) => true,
            Err(e) => {
                tracing::debug!("IPv6 interface multicast membership incomplete: {}", e);
                false
            }
        }
    }

    /// 先停止并等待 IPv6 接口的接收者退出，再释放套接字并退出该接口的组播组。
    async fn retire_v6(&mut self, index: u32) {
        if let Some(receiver) = self.receivers_v6.remove(&index) {
            receiver.stop().await;
        }
        self.sockets.remove_interface_v6(index);
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

    async fn set_v6(&mut self, desired: BTreeMap<u32, Ipv6Addr>) {
        self.desired_v6 = Some(desired);
        self.reconcile().await;
    }

    /// 让已安装接口与期望集合一致；仍有接口未安装时按有界退避安排下一次尝试。
    async fn reconcile(&mut self) {
        if self.desired.is_none() && self.desired_v6.is_none() {
            return;
        }
        let mut complete = true;
        if let Some(desired) = self.desired.clone() {
            let stale: Vec<Ipv4Addr> = self
                .receivers
                .keys()
                .filter(|addr| !desired.contains(addr))
                .copied()
                .collect();
            for addr in stale {
                self.retire(addr).await;
            }
            for addr in desired {
                complete &= self.install(addr);
            }
        }
        if let Some(desired) = self.desired_v6.clone() {
            let stale: Vec<u32> = self
                .receivers_v6
                .keys()
                .filter(|index| !desired.contains_key(index))
                .copied()
                .collect();
            for index in stale {
                self.retire_v6(index).await;
            }
            for (index, addr) in desired {
                complete &= self.install_v6(index, addr).await;
            }
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
            for receiver in self
                .receivers
                .values_mut()
                .chain(self.receivers_v6.values_mut())
            {
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
        receivers_v6: HashMap::new(),
        desired_v6: None,
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
            // IPv6 接口按接口索引维护，只经 `SetInterfacesV6` 修改；公开接口不发送按地址的
            // IPv6 增删。
            Input::AddInterface(IpAddr::V6(_)) | Input::RemoveInterface(IpAddr::V6(_)) => {}
            Input::SetInterfaces(desired) => interfaces.set(desired).await,
            Input::SetInterfacesV6(desired) => interfaces.set_v6(desired).await,
            msg => {
                snd_ref.send(sender::MdnsMsg::Update(msg));
            }
        }
    }
    drop(main_receivers);
}
