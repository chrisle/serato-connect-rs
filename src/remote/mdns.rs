//! Native DNS-SD publication, behind an injectable publisher. Constructing the
//! publisher/options or building records opens no sockets; publish is explicit.
use super::{RemoteError, RemoteResult};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
pub const SERATO_REMOTE_SERVICE_TYPE: &str = "SeratoIOSRemote";
pub const SERATO_REMOTE_SERVICE_DOMAIN: &str = "_SeratoIOSRemote._tcp.local.";
#[derive(Clone)]
pub struct MdnsPublishOptions {
    pub peer_name: String,
    pub port: u16,
    pub host: Option<String>,
    pub logger: std::sync::Arc<dyn crate::Logger>,
}
impl Default for MdnsPublishOptions {
    fn default() -> Self {
        Self {
            peer_name: "serato-connect".into(),
            port: 0,
            host: None,
            logger: crate::noop_logger(),
        }
    }
}
pub trait MdnsPublication: Send {
    fn instance_name(&self) -> &str;
    fn port(&self) -> u16;
    fn stop(&mut self) -> RemoteResult<()>;
}
pub trait MdnsPublisher: Send + Sync {
    fn publish(&self, options: &MdnsPublishOptions) -> RemoteResult<Box<dyn MdnsPublication>>;
}
#[derive(Default)]
pub struct NativeMdnsPublisher;
pub fn build_instance_name(peer_name: &str, host: Option<&str>) -> RemoteResult<String> {
    let host = match host {
        Some(host) => host.to_owned(),
        None => hostname::get()
            .map_err(|_| RemoteError::Publication)?
            .to_string_lossy()
            .into_owned(),
    };
    if peer_name.is_empty() || peer_name.contains('\0') || host.is_empty() || host.contains('\0') {
        return Err(RemoteError::InvalidData);
    }
    let name = format!("{peer_name} @ {host}");
    if name.len() > 63 {
        return Err(RemoteError::ResourceLimit);
    }
    Ok(name)
}
/// Build the exact service record without starting a responder (fixture safe).
pub fn service_info(options: &MdnsPublishOptions) -> RemoteResult<mdns_sd::ServiceInfo> {
    if options.port == 0 {
        return Err(RemoteError::InvalidData);
    }
    let host = match &options.host {
        Some(host) => host.clone(),
        None => hostname::get()
            .map_err(|_| RemoteError::Publication)?
            .to_string_lossy()
            .into_owned(),
    };
    let instance = build_instance_name(&options.peer_name, Some(&host))?;
    let host = host.trim_end_matches('.');
    let dns_host = if host.ends_with(".local") {
        format!("{host}.")
    } else {
        format!("{host}.local.")
    };
    mdns_sd::ServiceInfo::new(
        SERATO_REMOTE_SERVICE_DOMAIN,
        &instance,
        &dns_host,
        "",
        options.port,
        HashMap::<String, String>::new(),
    )
    .map(|info| info.enable_addr_auto())
    .map_err(|_| RemoteError::Publication)
}
pub fn publish_serato_remote(
    options: &MdnsPublishOptions,
) -> RemoteResult<Box<dyn MdnsPublication>> {
    NativeMdnsPublisher.publish(options)
}
impl MdnsPublisher for NativeMdnsPublisher {
    fn publish(&self, options: &MdnsPublishOptions) -> RemoteResult<Box<dyn MdnsPublication>> {
        let info = service_info(options)?;
        let instance = build_instance_name(&options.peer_name, options.host.as_deref())?;
        let full = info.get_fullname().to_owned();
        let daemon = mdns_sd::ServiceDaemon::new().map_err(|_| RemoteError::Publication)?;
        let mut publication = NativePublication {
            daemon: Some(daemon),
            instance,
            full,
            port: options.port,
            monitor_stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            monitor: None,
        };
        let daemon = publication.daemon.as_ref().unwrap();
        let events = daemon.monitor().map_err(|_| RemoteError::Publication)?;
        let stop = publication.monitor_stop.clone();
        let logger = options.logger.clone();
        publication.monitor = Some(
            std::thread::Builder::new()
                .name("serato-mdns-monitor".into())
                .spawn(move || {
                    while !stop.load(std::sync::atomic::Ordering::Acquire) {
                        match events.recv_timeout(Duration::from_millis(50)) {
                            Ok(mdns_sd::DaemonEvent::Error(_)) => {
                                logger.warn("Serato Remote discovery responder error")
                            }
                            Ok(_) => {}
                            Err(mdns_sd::RecvTimeoutError::Timeout) => {}
                            Err(_) => break,
                        }
                    }
                })
                .map_err(|_| RemoteError::Publication)?,
        );
        daemon
            .register(info)
            .map_err(|_| RemoteError::Publication)?;
        options
            .logger
            .info("Publishing Serato Remote discovery service");
        Ok(Box::new(publication))
    }
}
struct NativePublication {
    daemon: Option<mdns_sd::ServiceDaemon>,
    instance: String,
    full: String,
    port: u16,
    monitor_stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    monitor: Option<std::thread::JoinHandle<()>>,
}
impl MdnsPublication for NativePublication {
    fn instance_name(&self) -> &str {
        &self.instance
    }
    fn port(&self) -> u16 {
        self.port
    }
    fn stop(&mut self) -> RemoteResult<()> {
        self.monitor_stop
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.monitor.take() {
            let _ = thread.join();
        }
        let Some(daemon) = self.daemon.as_ref() else {
            return Ok(());
        };
        let deadline = Instant::now() + Duration::from_secs(1);
        if let Ok(done) = daemon.unregister(&self.full) {
            let _ = done.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        let done = loop {
            match daemon.shutdown() {
                Ok(done) => break done,
                Err(mdns_sd::Error::DaemonShutdown) => {
                    self.daemon = None;
                    return Ok(());
                }
                Err(mdns_sd::Error::Again) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => return Err(RemoteError::Publication),
            }
        };
        done.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| RemoteError::Deadline)?;
        self.daemon = None;
        Ok(())
    }
}
impl Drop for NativePublication {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
