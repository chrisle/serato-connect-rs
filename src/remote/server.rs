//! Bounded native TCP ownership. Serato initiates connections; no outbound
//! desktop connection or LAN discovery is performed by a server constructor.
use super::{arg, frame_osc, osc, FrameReader, OscArg, OscMessage, RemoteError, RemoteResult};
use md5::{Digest, Md5};
use serde::Serialize;
use std::{
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSessionPeer {
    pub remote_address: String,
    pub remote_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
}
impl RemoteSessionPeer {
    fn from_address(address: SocketAddr) -> Self {
        Self {
            remote_address: address.ip().to_string(),
            remote_port: address.port(),
            name: None,
            uuid: None,
        }
    }
}
#[derive(Clone, Debug)]
pub enum RemoteSessionEvent {
    Paired(RemoteSessionPeer),
    Status(OscMessage, RemoteSessionPeer),
    Ping(RemoteSessionPeer),
    Closed(RemoteSessionPeer),
    Error(RemoteError, RemoteSessionPeer),
}
pub struct RemoteSessionOptions {
    pub socket: TcpStream,
    pub subscribe_topics: Vec<String>,
    pub peer_name: String,
    pub peer_uuid: String,
    pub max_frame_bytes: usize,
    pub handshake_timeout: Duration,
    pub idle_timeout: Duration,
    pub logger: Arc<dyn crate::Logger>,
}
impl RemoteSessionOptions {
    pub fn new(
        socket: TcpStream,
        peer_name: impl Into<String>,
        peer_uuid: impl Into<String>,
        subscribe_topics: Vec<String>,
    ) -> Self {
        Self {
            socket,
            subscribe_topics,
            peer_name: peer_name.into(),
            peer_uuid: peer_uuid.into(),
            max_frame_bytes: 1 << 20,
            handshake_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(60),
            logger: crate::noop_logger(),
        }
    }
}
struct Shared {
    peer: Mutex<RemoteSessionPeer>,
    paired: AtomicBool,
    running: AtomicBool,
    stop: AtomicBool,
    last_error: Mutex<Option<RemoteError>>,
    activated: AtomicBool,
}
pub struct RemoteSession {
    shared: Arc<Shared>,
    sender: SyncSender<Vec<u8>>,
    events: Mutex<Receiver<RemoteSessionEvent>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    max: usize,
}
impl RemoteSession {
    pub fn new(options: RemoteSessionOptions) -> RemoteResult<Self> {
        Self::build(options, None)
    }
    fn build(
        mut options: RemoteSessionOptions,
        forward: Option<(u64, SyncSender<RemoteServerEvent>)>,
    ) -> RemoteResult<Self> {
        validate_options(&options)?;
        let peer = RemoteSessionPeer::from_address(
            options.socket.peer_addr().map_err(|_| RemoteError::Io)?,
        );
        options
            .socket
            .set_nodelay(true)
            .map_err(|_| RemoteError::Io)?;
        options
            .socket
            .set_read_timeout(Some(Duration::from_millis(20)))
            .map_err(|_| RemoteError::Io)?;
        options
            .socket
            .set_write_timeout(Some(Duration::from_millis(250)))
            .map_err(|_| RemoteError::Io)?;
        let shared = Arc::new(Shared {
            peer: Mutex::new(peer),
            paired: AtomicBool::new(false),
            running: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            last_error: Mutex::new(None),
            activated: AtomicBool::new(forward.is_none()),
        });
        let (sender, commands) = mpsc::sync_channel::<Vec<u8>>(16);
        let (events, receive) = mpsc::sync_channel(256);
        let state = shared.clone();
        let max = options.max_frame_bytes;
        let worker = thread::Builder::new()
            .name("serato-remote-session".into())
            .spawn(move || {
                struct Lease(Arc<Shared>);
                impl Drop for Lease {
                    fn drop(&mut self) {
                        self.0.running.store(false, Ordering::Release);
                        self.0.paired.store(false, Ordering::Release);
                    }
                }
                let _lease = Lease(state.clone());
                while !state.activated.load(Ordering::Acquire)
                    && !state.stop.load(Ordering::Acquire)
                {
                    thread::sleep(Duration::from_millis(1));
                }
                let mut machine = SessionMachine {
                    peer_name: options.peer_name,
                    peer_uuid: options.peer_uuid,
                    topics: options.subscribe_topics,
                    subscribed: false,
                    paired: false,
                };
                let mut reader = FrameReader::new(options.max_frame_bytes);
                let started = Instant::now();
                let mut last = started;
                let mut bytes = [0u8; 8192];
                let emit = |event: RemoteSessionEvent| -> RemoteResult<()> {
                    if let Some((id, forward)) = &forward {
                        forward
                            .try_send(RemoteServerEvent::SessionEvent {
                                session_id: *id,
                                event,
                            })
                            .map_err(|_| RemoteError::ResourceLimit)
                    } else {
                        events
                            .try_send(event)
                            .map_err(|_| RemoteError::ResourceLimit)
                    }
                };
                let result = (|| -> RemoteResult<()> {
                    loop {
                        if state.stop.load(Ordering::Acquire) {
                            if machine.paired {
                                let _ = options
                                    .socket
                                    .write_all(&frame_osc(&osc("/StreamMgmt/Pairing/UnPair", []))?);
                            }
                            return Ok(());
                        }
                        if !machine.paired && started.elapsed() >= options.handshake_timeout
                            || last.elapsed() >= options.idle_timeout
                        {
                            return Err(RemoteError::Deadline);
                        }
                        for _ in 0..16 {
                            if state.stop.load(Ordering::Acquire) {
                                break;
                            }
                            match commands.try_recv() {
                                Ok(frame) => options
                                    .socket
                                    .write_all(&frame)
                                    .map_err(|_| RemoteError::Io)?,
                                Err(TryRecvError::Empty) => break,
                                Err(TryRecvError::Disconnected) => return Ok(()),
                            }
                        }
                        match options.socket.read(&mut bytes) {
                            Ok(0) => return Ok(()),
                            Ok(count) => {
                                last = Instant::now();
                                for message in reader.push(&bytes[..count])? {
                                    let actions =
                                        machine.dispatch(message, &state.peer.lock().unwrap());
                                    for action in actions {
                                        if state.stop.load(Ordering::Acquire) {
                                            return Ok(());
                                        }
                                        match action {
                                            Action::Send(message) => options
                                                .socket
                                                .write_all(&frame_osc(&message)?)
                                                .map_err(|_| RemoteError::Io)?,
                                            Action::Pair(peer) => {
                                                *state.peer.lock().unwrap() = peer.clone();
                                                state.paired.store(true, Ordering::Release);
                                                emit(RemoteSessionEvent::Paired(peer))?;
                                            }
                                            Action::Status(message) => {
                                                emit(RemoteSessionEvent::Status(
                                                    message,
                                                    state.peer.lock().unwrap().clone(),
                                                ))?
                                            }
                                            Action::Ping => emit(RemoteSessionEvent::Ping(
                                                state.peer.lock().unwrap().clone(),
                                            ))?,
                                            Action::Close => return Ok(()),
                                        }
                                    }
                                }
                            }
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::TimedOut
                                        | std::io::ErrorKind::Interrupted
                                ) => {}
                            Err(_) => return Err(RemoteError::Io),
                        }
                    }
                })();
                if let Err(error) = result {
                    options.logger.warn("Serato Remote session failed");
                    *state.last_error.lock().unwrap() = Some(error);
                    let _ = emit(RemoteSessionEvent::Error(
                        error,
                        state.peer.lock().unwrap().clone(),
                    ));
                }
                let _ = options.socket.shutdown(Shutdown::Both);
                state.paired.store(false, Ordering::Release);
                state.running.store(false, Ordering::Release);
                // Like the original explicit close(), local teardown does not
                // report a spontaneous peer-disconnect event to its caller.
                if !state.stop.load(Ordering::Acquire) {
                    let _ = emit(RemoteSessionEvent::Closed(
                        state.peer.lock().unwrap().clone(),
                    ));
                }
            })
            .map_err(|_| RemoteError::Io)?;
        Ok(Self {
            shared,
            sender,
            events: Mutex::new(receive),
            thread: Mutex::new(Some(worker)),
            max,
        })
    }
    pub fn remote_peer(&self) -> RemoteSessionPeer {
        self.shared.peer.lock().unwrap().clone()
    }
    pub fn paired(&self) -> bool {
        self.shared.paired.load(Ordering::Acquire)
    }
    pub fn running(&self) -> bool {
        self.shared.running.load(Ordering::Acquire)
    }
    pub fn last_error(&self) -> Option<RemoteError> {
        *self.shared.last_error.lock().unwrap()
    }
    pub fn try_recv(&self) -> std::result::Result<RemoteSessionEvent, TryRecvError> {
        self.events.lock().unwrap().try_recv()
    }
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<RemoteSessionEvent, mpsc::RecvTimeoutError> {
        self.events.lock().unwrap().recv_timeout(timeout)
    }
    pub fn send(&self, message: &OscMessage) -> RemoteResult<()> {
        if !self.running() {
            return Err(RemoteError::Stopped);
        }
        let frame = frame_osc(message)?;
        if frame.len() > self.max.saturating_add(16) {
            return Err(RemoteError::ResourceLimit);
        }
        self.sender
            .try_send(frame)
            .map_err(|_| RemoteError::ResourceLimit)
    }
    pub fn close(&self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(worker) = self.thread.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}
impl Drop for RemoteSession {
    fn drop(&mut self) {
        self.close()
    }
}
fn validate_options(options: &RemoteSessionOptions) -> RemoteResult<()> {
    if options.peer_name.len() > 1024
        || options.peer_uuid.len() > 1024
        || options.peer_name.contains('\0')
        || options.peer_uuid.contains('\0')
        || options.subscribe_topics.len() > 64
        || options
            .subscribe_topics
            .iter()
            .any(|s| s.len() > 4096 || s.contains('\0'))
        || options.max_frame_bytes == 0
        || options.max_frame_bytes > 16 << 20
        || options.handshake_timeout.is_zero()
        || options.idle_timeout.is_zero()
    {
        Err(RemoteError::ResourceLimit)
    } else {
        Ok(())
    }
}
#[derive(Clone)]
pub struct RemoteServerOptions {
    pub port: u16,
    pub host: String,
    pub subscribe_topics: Vec<String>,
    pub peer_name: String,
    pub peer_uuid: Option<String>,
    pub max_frame_bytes: usize,
    pub max_peers: usize,
    pub handshake_timeout: Duration,
    pub idle_timeout: Duration,
    pub logger: Arc<dyn crate::Logger>,
}
impl Default for RemoteServerOptions {
    fn default() -> Self {
        Self {
            port: 0,
            host: "0.0.0.0".into(),
            subscribe_topics: super::DEFAULT_SUBSCRIPTION_TOPICS
                .iter()
                .map(|s| (*s).into())
                .collect(),
            peer_name: "serato-connect".into(),
            peer_uuid: None,
            max_frame_bytes: 1 << 20,
            max_peers: 32,
            handshake_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(60),
            logger: crate::noop_logger(),
        }
    }
}
pub enum RemoteServerEvent {
    Listening {
        port: u16,
        host: String,
    },
    Session {
        session_id: u64,
        session: Arc<RemoteSession>,
    },
    SessionEvent {
        session_id: u64,
        event: RemoteSessionEvent,
    },
    Error(RemoteError),
}
pub struct RemoteServer {
    options: RemoteServerOptions,
    resolver: Arc<dyn super::HostResolver>,
    peer_uuid: String,
    port: Option<u16>,
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    events: Receiver<RemoteServerEvent>,
    sender: SyncSender<RemoteServerEvent>,
    thread: Option<JoinHandle<()>>,
}
impl RemoteServer {
    pub fn new(options: RemoteServerOptions) -> Self {
        Self::with_resolver(options, Arc::new(super::NativeHostResolver))
    }
    /// Supply a deadline-respecting resolver for controlled fixtures/embedding.
    /// Constructing the server does not invoke it or open a listener.
    pub fn with_resolver(
        options: RemoteServerOptions,
        resolver: Arc<dyn super::HostResolver>,
    ) -> Self {
        let peer_uuid = options
            .peer_uuid
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let (sender, events) = mpsc::sync_channel(512);
        Self {
            options,
            resolver,
            peer_uuid,
            port: None,
            stop: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            events,
            sender,
            thread: None,
        }
    }
    pub fn port(&self) -> RemoteResult<u16> {
        self.port.ok_or(RemoteError::Stopped)
    }
    pub fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
    pub fn events(&self) -> &Receiver<RemoteServerEvent> {
        &self.events
    }
    pub fn start(&mut self) -> RemoteResult<u16> {
        if self.running() {
            return Err(RemoteError::AlreadyRunning);
        }
        if self.thread.is_some() {
            self.stop()
        }
        while self.events.try_recv().is_ok() {}
        if self.options.host.len() > 1024 || self.options.host.contains('\0') {
            return Err(RemoteError::ResourceLimit);
        }
        if self.options.max_peers == 0
            || self.options.max_peers > 128
            || self.options.max_frame_bytes == 0
            || self.options.max_frame_bytes > 16 << 20
            || self.options.peer_name.len() > 1024
            || self.peer_uuid.len() > 1024
            || self.options.peer_name.contains('\0')
            || self.peer_uuid.contains('\0')
            || self.options.subscribe_topics.len() > 64
            || self
                .options
                .subscribe_topics
                .iter()
                .any(|s| s.len() > 4096 || s.contains('\0'))
            || self.options.handshake_timeout.is_zero()
            || self.options.idle_timeout.is_zero()
        {
            return Err(RemoteError::ResourceLimit);
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        let addresses = if let Some(ip) = super::numeric_host_address(&self.options.host) {
            vec![SocketAddr::new(ip, self.options.port)]
        } else {
            self.resolver
                .resolve(&self.options.host, self.options.port, deadline)?
        };
        if addresses.is_empty() {
            return Err(RemoteError::Io);
        }
        if addresses.len() > super::MAX_RESOLVED_ADDRESSES {
            return Err(RemoteError::ResourceLimit);
        }
        let mut listener = None;
        for address in addresses {
            if Instant::now() >= deadline {
                return Err(RemoteError::Deadline);
            }
            if let Ok(bound) = TcpListener::bind(address) {
                listener = Some(bound);
                break;
            }
        }
        let listener = listener.ok_or(RemoteError::Io)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| RemoteError::Io)?;
        let port = listener.local_addr().map_err(|_| RemoteError::Io)?.port();
        self.stop.store(false, Ordering::Release);
        self.running.store(true, Ordering::Release);
        self.port = Some(port);
        let (stop, running, sender, options, uuid) = (
            self.stop.clone(),
            self.running.clone(),
            self.sender.clone(),
            self.options.clone(),
            self.peer_uuid.clone(),
        );
        self.thread = Some(
            thread::Builder::new()
                .name("serato-remote-listener".into())
                .spawn(move || {
                    struct Lease(Arc<AtomicBool>);
                    impl Drop for Lease {
                        fn drop(&mut self) {
                            self.0.store(false, Ordering::Release)
                        }
                    }
                    let _lease = Lease(running);
                    let mut sessions: Vec<Arc<RemoteSession>> = Vec::new();
                    let next = AtomicU64::new(0);
                    let _ = sender.try_send(RemoteServerEvent::Listening {
                        port,
                        host: options.host.clone(),
                    });
                    while !stop.load(Ordering::Acquire) {
                        sessions.retain(|s| s.running());
                        match listener.accept() {
                            Ok((socket, _)) => {
                                if sessions.len() >= options.max_peers {
                                    let _ = socket.shutdown(Shutdown::Both);
                                    let _ = sender.try_send(RemoteServerEvent::Error(
                                        RemoteError::ResourceLimit,
                                    ));
                                    continue;
                                }
                                let mut session_options = RemoteSessionOptions::new(
                                    socket,
                                    options.peer_name.clone(),
                                    uuid.clone(),
                                    options.subscribe_topics.clone(),
                                );
                                session_options.max_frame_bytes = options.max_frame_bytes;
                                session_options.handshake_timeout = options.handshake_timeout;
                                session_options.idle_timeout = options.idle_timeout;
                                session_options.logger = options.logger.clone();
                                let id = next.fetch_add(1, Ordering::Relaxed) + 1;
                                match RemoteSession::build(
                                    session_options,
                                    Some((id, sender.clone())),
                                ) {
                                    Ok(session) => {
                                        let session = Arc::new(session);
                                        if sender
                                            .try_send(RemoteServerEvent::Session {
                                                session_id: id,
                                                session: session.clone(),
                                            })
                                            .is_err()
                                        {
                                            session.close();
                                            break;
                                        }
                                        session.shared.activated.store(true, Ordering::Release);
                                        sessions.push(session)
                                    }
                                    Err(error) => {
                                        if sender.try_send(RemoteServerEvent::Error(error)).is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(5))
                            }
                            Err(_) => {
                                let _ = sender.try_send(RemoteServerEvent::Error(RemoteError::Io));
                                break;
                            }
                        }
                    }
                    for session in &sessions {
                        session.shared.stop.store(true, Ordering::Release);
                    }
                    for session in sessions {
                        session.close()
                    }
                })
                .map_err(|_| {
                    self.running.store(false, Ordering::Release);
                    self.port = None;
                    RemoteError::Io
                })?,
        );
        Ok(port)
    }
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.running.store(false, Ordering::Release);
        self.port = None;
        while self.events.try_recv().is_ok() {}
    }
}
impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.stop()
    }
}
enum Action {
    Send(OscMessage),
    Pair(RemoteSessionPeer),
    Status(OscMessage),
    Ping,
    Close,
}
struct SessionMachine {
    peer_name: String,
    peer_uuid: String,
    topics: Vec<String>,
    subscribed: bool,
    paired: bool,
}
impl SessionMachine {
    fn subscribe(&mut self, out: &mut Vec<Action>) {
        if self.subscribed {
            return;
        }
        self.subscribed = true;
        for topic in &self.topics {
            out.push(Action::Send(osc(topic, [])))
        }
    }
    fn dispatch(&mut self, message: OscMessage, peer: &RemoteSessionPeer) -> Vec<Action> {
        let mut out = Vec::new();
        match message.address.as_str() {
            "/StreamMgmt/Authorize/Request" => {
                if let Some(nonce) = message.args.iter().find_map(|a| {
                    if let OscArg::Blob(v) = a {
                        Some(v)
                    } else {
                        None
                    }
                }) {
                    let mut hash = Md5::new();
                    hash.update(nonce);
                    hash.update([
                        0xd2, 0x5d, 0xb2, 0x61, 0xa4, 0x41, 0x1b, 0xc1, 0xf3, 0x78, 0x8f, 0x57,
                        0x72, 0x3b, 0x89, 0x77, 0xc5, 0x41, 0xa4, 0xb6, 0x19, 0xb9, 0x4a, 0x9a,
                        0x8b, 0x45, 0xc4, 0x6f, 0x85, 0x11, 0xc2, 0xf8,
                    ]);
                    out.push(Action::Send(osc(
                        "/StreamMgmt/Authorize/Response",
                        [
                            arg::s(&self.peer_name),
                            arg::s(&self.peer_uuid),
                            arg::b(hash.finalize().to_vec()),
                        ],
                    )));
                }
            }
            "/StreamMgmt/Pairing/Pair" => {
                let mut peer = peer.clone();
                if let Some(OscArg::String(v)) = message.args.first() {
                    if !v.is_empty() {
                        peer.name = Some(v.clone())
                    }
                }
                if let Some(OscArg::String(v)) = message.args.get(1) {
                    if !v.is_empty() {
                        peer.uuid = Some(v.clone())
                    }
                }
                out.push(Action::Send(osc(
                    "/StreamMgmt/Pairing/Pair",
                    [arg::s(&self.peer_name), arg::s(&self.peer_uuid), arg::i(1)],
                )));
                self.subscribe(&mut out);
                if !self.paired {
                    self.paired = true;
                    out.push(Action::Pair(peer));
                }
            }
            "/StreamMgmt/Pairing/StatusChanged" => self.subscribe(&mut out),
            "/StreamMgmt/Pairing/UnPair" => {
                self.paired = false;
                out.push(Action::Close)
            }
            "/Ping" => {
                out.push(Action::Send(osc("/Ping", [])));
                out.push(Action::Ping)
            }
            address if address.starts_with("/Status/") => out.push(Action::Status(message)),
            _ => {}
        }
        out
    }
}

pub type RemoteSessionEvents = RemoteSessionEvent;
pub type RemoteServerEvents = RemoteServerEvent;
