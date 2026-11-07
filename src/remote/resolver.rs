//! Bounded native hostname resolution for TCP listener startup. Numeric IP
//! literals never enter a worker or DNS. Native hostname lookups share one lazy
//! worker across all servers/retries: OS getaddrinfo cannot be interrupted, so
//! a stalled call occupies that one worker rather than spawning more threads.
use super::{RemoteError, RemoteResult};
use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex, OnceLock,
    },
    thread::{self, JoinHandle},
    time::Instant,
};
pub const MAX_RESOLVED_ADDRESSES: usize = 32;
pub const RESOLVER_QUEUE_CAPACITY: usize = 8;
/// Implementations must return by `deadline`, without logging raw hostnames or
/// creating an unbounded population of outstanding native lookup workers.
pub trait HostResolver: Send + Sync {
    fn resolve(&self, host: &str, port: u16, deadline: Instant) -> RemoteResult<Vec<SocketAddr>>;
}
pub type HostLookup = Arc<dyn Fn(&str, u16) -> RemoteResult<Vec<SocketAddr>> + Send + Sync>;
struct Request {
    host: String,
    port: u16,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    reply: SyncSender<RemoteResult<Vec<SocketAddr>>>,
}
#[derive(Default)]
struct Worker {
    sender: Option<SyncSender<Request>>,
    thread: Option<JoinHandle<()>>,
}
/// An injectable lookup uses the same fixed worker, queue and cancellation
/// behavior as native resolution. Construct once and reuse it across retries.
/// Construction itself starts no thread and performs no hostname lookup.
pub struct BoundedResolver {
    lookup: HostLookup,
    worker: Mutex<Worker>,
    closed: Arc<AtomicBool>,
    shutdown_lock: Mutex<()>,
}
impl BoundedResolver {
    pub fn with_lookup(lookup: HostLookup) -> Self {
        Self {
            lookup,
            worker: Mutex::new(Worker::default()),
            closed: Arc::new(AtomicBool::new(false)),
            shutdown_lock: Mutex::new(()),
        }
    }
    fn sender(&self) -> RemoteResult<SyncSender<Request>> {
        let mut worker = self.worker.lock().map_err(|_| RemoteError::Io)?;
        if self.closed.load(Ordering::Acquire) {
            return Err(RemoteError::Stopped);
        }
        if let Some(sender) = &worker.sender {
            return Ok(sender.clone());
        }
        let (sender, receive) = mpsc::sync_channel::<Request>(RESOLVER_QUEUE_CAPACITY);
        let lookup = self.lookup.clone();
        let closed = self.closed.clone();
        let thread = thread::Builder::new()
            .name("serato-host-resolver".into())
            .spawn(move || {
                while let Ok(request) = receive.recv() {
                    if closed.load(Ordering::Acquire) {
                        let _ = request.reply.try_send(Err(RemoteError::Stopped));
                        continue;
                    }
                    if request.cancelled.load(Ordering::Acquire)
                        || Instant::now() >= request.deadline
                    {
                        continue;
                    }
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        lookup(&request.host, request.port)
                    }))
                    .unwrap_or(Err(RemoteError::Io))
                    .and_then(validate_output);
                    if request.cancelled.load(Ordering::Acquire)
                        || Instant::now() >= request.deadline
                    {
                        continue;
                    }
                    let _ = request.reply.try_send(result);
                }
            })
            .map_err(|_| RemoteError::Io)?;
        worker.thread = Some(thread);
        worker.sender = Some(sender.clone());
        Ok(sender)
    }
}
impl BoundedResolver {
    /// Stop accepting work and join this injected worker when it can finish.
    /// A stalled native lookup cannot be interrupted; a timeout reports that
    /// cleanup is unconfirmed and never starts a replacement worker.
    pub fn shutdown(&self, deadline: Instant) -> RemoteResult<()> {
        let _shutdown = self
            .shutdown_lock
            .try_lock()
            .map_err(|_| RemoteError::ResourceLimit)?;
        self.closed.store(true, Ordering::Release);
        let thread = {
            let mut worker = self.worker.lock().map_err(|_| RemoteError::Io)?;
            worker.sender.take();
            worker.thread.take()
        };
        if let Some(thread) = thread {
            while !thread.is_finished() {
                if Instant::now() >= deadline {
                    self.worker.lock().map_err(|_| RemoteError::Io)?.thread = Some(thread);
                    return Err(RemoteError::Deadline);
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            thread.join().map_err(|_| RemoteError::Io)?;
        }
        Ok(())
    }
}
impl HostResolver for BoundedResolver {
    fn resolve(&self, host: &str, port: u16, deadline: Instant) -> RemoteResult<Vec<SocketAddr>> {
        if self.closed.load(Ordering::Acquire) {
            return Err(RemoteError::Stopped);
        }
        if Instant::now() >= deadline {
            return Err(RemoteError::Deadline);
        }
        validate_host(host)?;
        if let Some(ip) = numeric_host_address(host) {
            return Ok(vec![SocketAddr::new(ip, port)]);
        }
        let (reply, receive) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.sender()?
            .try_send(Request {
                host: host.into(),
                port,
                deadline,
                cancelled: cancelled.clone(),
                reply,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => RemoteError::ResourceLimit,
                mpsc::TrySendError::Disconnected(_) => RemoteError::Io,
            })?;
        match receive.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(_) if self.closed.load(Ordering::Acquire) => Err(RemoteError::Stopped),
            Ok(result) if Instant::now() < deadline => result,
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {
                cancelled.store(true, Ordering::Release);
                Err(RemoteError::Deadline)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                cancelled.store(true, Ordering::Release);
                Err(RemoteError::Io)
            }
        }
    }
}
impl Drop for BoundedResolver {
    fn drop(&mut self) {
        if let Ok(worker) = self.worker.get_mut() {
            worker.sender.take();
            if let Some(thread) = worker.thread.take() {
                // Idle/completed workers can be reaped immediately. A currently
                // blocked OS getaddrinfo cannot be forcibly stopped safely.
                // Native use is the process-wide static below, never one worker
                // per request/retry. Once its lookup returns, channel closure
                // makes this owned worker exit without executing stale requests.
                if thread.is_finished() {
                    let _ = thread.join();
                }
            }
        }
    }
}
fn validate_host(host: &str) -> RemoteResult<()> {
    if host.is_empty() || host.contains('\0') {
        return Err(RemoteError::InvalidData);
    }
    if host.len() > 1024 {
        return Err(RemoteError::ResourceLimit);
    }
    Ok(())
}
fn validate_output(addresses: Vec<SocketAddr>) -> RemoteResult<Vec<SocketAddr>> {
    if addresses.len() > MAX_RESOLVED_ADDRESSES {
        return Err(RemoteError::ResourceLimit);
    }
    if addresses.is_empty() {
        return Err(RemoteError::Io);
    }
    Ok(addresses)
}
#[derive(Default)]
pub struct NativeHostResolver;
impl HostResolver for NativeHostResolver {
    fn resolve(&self, host: &str, port: u16, deadline: Instant) -> RemoteResult<Vec<SocketAddr>> {
        if Instant::now() >= deadline {
            return Err(RemoteError::Deadline);
        }
        validate_host(host)?;
        if let Some(ip) = numeric_host_address(host) {
            return Ok(vec![SocketAddr::new(ip, port)]);
        }
        static RESOLVER: OnceLock<BoundedResolver> = OnceLock::new();
        RESOLVER
            .get_or_init(|| {
                BoundedResolver::with_lookup(Arc::new(|host, port| {
                    let addresses = (host, port)
                        .to_socket_addrs()
                        .map_err(|_| RemoteError::Io)?
                        .take(MAX_RESOLVED_ADDRESSES + 1)
                        .collect();
                    validate_output(addresses)
                }))
            })
            .resolve(host, port, deadline)
    }
}

/// Parse IP literals and the legacy IPv4 numbers-and-dots forms accepted by
/// native getaddrinfo/inet_aton. This makes valid numeric aliases (127.1,
/// 2130706433, 0x7f000001) local-only operations with no DNS fallback.
pub fn numeric_host_address(host: &str) -> Option<IpAddr> {
    if host.len() > 1024 {
        return None;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(ip);
    }
    let parts: Vec<_> = host.split('.').take(5).collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut values = Vec::with_capacity(parts.len());
    for part in parts {
        if part.is_empty() {
            return None;
        }
        let (digits, radix) =
            if let Some(part) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
                (part, 16)
            } else if part.len() > 1 && part.starts_with('0') {
                (&part[1..], 8)
            } else {
                (part, 10)
            };
        if digits.is_empty()
            || !digits.bytes().all(|b| match radix {
                16 => b.is_ascii_hexdigit(),
                8 => matches!(b, b'0'..=b'7'),
                _ => b.is_ascii_digit(),
            })
        {
            return None;
        }
        values.push(u32::from_str_radix(digits, radix).ok()?);
    }
    let value = match values.as_slice() {
        [a] => *a,
        [a, b] if *a <= 255 && *b <= 0x00ff_ffff => (a << 24) | b,
        [a, b, c] if *a <= 255 && *b <= 255 && *c <= 0xffff => (a << 24) | (b << 16) | c,
        [a, b, c, d] if values.iter().all(|v| *v <= 255) => (a << 24) | (b << 16) | (c << 8) | d,
        _ => return None,
    };
    Some(IpAddr::V4(std::net::Ipv4Addr::from(value)))
}
