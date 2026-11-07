use serato_connect_rs::*;
use std::{
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
fn wait(mut predicate: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(3);
    while !predicate() {
        assert!(Instant::now() < until, "remote fixture deadline");
        thread::sleep(Duration::from_millis(5));
    }
}
fn loopback() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (socket, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_millis(40)))
        .unwrap();
    peer.set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    (socket, peer)
}
fn frames(peer: &mut TcpStream, count: usize) -> Vec<OscMessage> {
    let mut reader = FrameReader::default();
    let mut output = Vec::new();
    let until = Instant::now() + Duration::from_secs(2);
    while output.len() < count {
        assert!(Instant::now() < until, "TCP response fixture deadline");
        let mut buf = [0; 8192];
        match peer.read(&mut buf) {
            Ok(0) => break,
            Ok(count) => output.extend(reader.push(&buf[..count]).unwrap()),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => panic!("fixture read: {e}"),
        }
    }
    output
}
fn session_options(socket: TcpStream) -> RemoteSessionOptions {
    RemoteSessionOptions::new(
        socket,
        "Synthetic Remote",
        "synthetic-peer-uuid",
        vec!["/Register/Status/Deck/Playhead".into()],
    )
}
fn assert_peer_closed(peer: &mut TcpStream) {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        assert!(Instant::now() < until, "peer not closed");
        let mut data = [0; 4096];
        match peer.read(&mut data) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                ) =>
            {
                break
            }
            Err(e) => panic!("fixture close: {e}"),
        }
    }
}
#[test]
fn loopback_session_authorize_pair_subscribe_ping_and_unpair_match_actual_ts() {
    let (socket, mut peer) = loopback();
    let session = RemoteSession::new(session_options(socket)).unwrap();
    let authorization = frame_osc(&osc(
        "/StreamMgmt/Authorize/Request",
        [arg::b((0..16).collect::<Vec<u8>>())],
    ))
    .unwrap();
    for byte in authorization {
        peer.write_all(&[byte]).unwrap();
    }
    peer.write_all(
        &frame_osc(&osc(
            "/StreamMgmt/Pairing/Pair",
            [arg::s("Synthetic Serato"), arg::s("Serato DJ"), arg::i(0)],
        ))
        .unwrap(),
    )
    .unwrap();
    peer.write_all(&frame_osc(&osc("/Ping", [])).unwrap())
        .unwrap();
    let mut output = frames(&mut peer, 4);
    assert_eq!(output.len(), 4);
    wait(|| session.paired());
    assert_eq!(
        session.remote_peer().name.as_deref(),
        Some("Synthetic Serato")
    );
    assert_eq!(session.remote_peer().uuid.as_deref(), Some("Serato DJ"));
    session.close();
    output.extend(frames(&mut peer, 1));
    assert_eq!(output.len(), 5);
    assert!(!session.running());
    assert!(!session.paired());
    let actual = serde_json::to_value(output).unwrap();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../tools/runtime/expected.json")).unwrap();
    assert_eq!(actual, expected["sessionFrames"]);
    assert_peer_closed(&mut peer);
}
#[test]
fn standalone_session_forwards_status_and_closes_malformed_deadline_and_overflow() {
    let (socket, mut peer) = loopback();
    let session = RemoteSession::new(session_options(socket)).unwrap();
    let message = osc(
        "/Status/Deck/Song/Title",
        [arg::i(0), arg::s("Synthetic Track")],
    );
    peer.write_all(&frame_osc(&message).unwrap()).unwrap();
    assert!(
        matches!(session.recv_timeout(Duration::from_secs(1)).unwrap(),RemoteSessionEvent::Status(received,_) if received==message)
    );
    session.close();
    assert_peer_closed(&mut peer);
    let (socket, mut peer) = loopback();
    let mut options = session_options(socket);
    options.max_frame_bytes = 32;
    let session = RemoteSession::new(options).unwrap();
    peer.write_all(&[b'x'; 48]).unwrap();
    wait(|| !session.running());
    assert_eq!(session.last_error(), Some(RemoteError::ResourceLimit));
    session.close();
    assert_peer_closed(&mut peer);
    let (socket, mut peer) = loopback();
    let mut options = session_options(socket);
    options.handshake_timeout = Duration::from_millis(80);
    let session = RemoteSession::new(options).unwrap();
    wait(|| !session.running());
    assert_eq!(session.last_error(), Some(RemoteError::Deadline));
    session.close();
    assert_peer_closed(&mut peer);
    let (socket, mut peer) = loopback();
    let session = RemoteSession::new(session_options(socket)).unwrap();
    let flood = frame_osc(&osc("/Ping", [])).unwrap().repeat(300);
    peer.write_all(&flood).unwrap();
    wait(|| !session.running());
    assert_eq!(session.last_error(), Some(RemoteError::ResourceLimit));
    session.close();
    assert_peer_closed(&mut peer);
}
#[test]
fn native_server_accepts_two_streams_bounds_peers_and_closes_actual_sockets() {
    let mut server = RemoteServer::new(RemoteServerOptions {
        host: "127.0.0.1".into(),
        max_peers: 2,
        ..Default::default()
    });
    assert!(server.port().is_err());
    assert!(!server.running());
    let port = server.start().unwrap();
    let mut first = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut second = TcpStream::connect(("127.0.0.1", port)).unwrap();
    for peer in [&first, &second] {
        peer.set_read_timeout(Some(Duration::from_millis(40)))
            .unwrap();
    }
    let mut sessions = Vec::new();
    let until = Instant::now() + Duration::from_secs(2);
    while sessions.len() < 2 {
        assert!(Instant::now() < until);
        if let Ok(RemoteServerEvent::Session { session, .. }) =
            server.events().recv_timeout(Duration::from_millis(50))
        {
            sessions.push(session);
        }
    }
    let mut extra = TcpStream::connect(("127.0.0.1", port)).unwrap();
    extra
        .set_read_timeout(Some(Duration::from_millis(40)))
        .unwrap();
    assert_peer_closed(&mut extra);
    let before = Instant::now();
    server.stop();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(!server.running());
    assert!(server.port().is_err());
    for session in sessions {
        assert!(!session.running());
        assert_eq!(session.send(&osc("/Ping", [])), Err(RemoteError::Stopped));
    }
    assert_peer_closed(&mut first);
    assert_peer_closed(&mut second);
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    let next = server.start().unwrap();
    assert!(next > 0);
    server.stop();
}
#[derive(Default)]
struct Publisher {
    calls: AtomicUsize,
    stops: Arc<AtomicUsize>,
    ports: Mutex<Vec<u16>>,
    fail: bool,
}
struct Publication {
    name: String,
    port: u16,
    stops: Arc<AtomicUsize>,
    stopped: bool,
}
impl MdnsPublication for Publication {
    fn instance_name(&self) -> &str {
        &self.name
    }
    fn port(&self) -> u16 {
        self.port
    }
    fn stop(&mut self) -> RemoteResult<()> {
        if !self.stopped {
            self.stops.fetch_add(1, Ordering::Relaxed);
            self.stopped = true;
        }
        Ok(())
    }
}
impl Drop for Publication {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
impl MdnsPublisher for Publisher {
    fn publish(&self, options: &MdnsPublishOptions) -> RemoteResult<Box<dyn MdnsPublication>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.ports.lock().unwrap().push(options.port);
        if self.fail {
            return Err(RemoteError::Publication);
        }
        Ok(Box::new(Publication {
            name: build_instance_name(&options.peer_name, Some("fixture.local"))?,
            port: options.port,
            stops: self.stops.clone(),
            stopped: false,
        }))
    }
}
fn client_options() -> SeratoRemoteOptions {
    SeratoRemoteOptions {
        host: "127.0.0.1".into(),
        peer_name: "Synthetic Remote".into(),
        mdns_host: Some("fixture.local".into()),
        ..Default::default()
    }
}
#[test]
fn injected_publication_client_pairs_settles_state_and_releases_on_stop() {
    let publisher = Arc::new(Publisher::default());
    let mut client = SeratoRemoteClient::with_publisher(client_options(), publisher.clone());
    assert!(!client.running());
    assert_eq!(publisher.calls.load(Ordering::Relaxed), 0);
    let ready = client.start().unwrap();
    assert!(ready.port > 0);
    assert_eq!(ready.instance_name, "Synthetic Remote @ fixture.local");
    assert!(client.start().is_err());
    let mut peer = TcpStream::connect(("127.0.0.1", ready.port)).unwrap();
    peer.set_read_timeout(Some(Duration::from_millis(40)))
        .unwrap();
    peer.write_all(
        &frame_osc(&osc(
            "/StreamMgmt/Pairing/Pair",
            [arg::s("Synthetic Serato"), arg::s("Serato DJ"), arg::i(0)],
        ))
        .unwrap(),
    )
    .unwrap();
    for (address, value) in [
        ("Title", "Settled Song"),
        ("Artist", "Settled Artist"),
        ("Filepath", "/Synthetic/settled.mp3"),
    ] {
        peer.write_all(
            &frame_osc(&osc(
                format!("/Status/Deck/Song/{address}"),
                [arg::i(0), arg::s(value)],
            ))
            .unwrap(),
        )
        .unwrap();
    }
    let until = Instant::now() + Duration::from_secs(2);
    let (mut connected, mut paired, mut changes) = (false, false, 0);
    loop {
        assert!(Instant::now() < until);
        match client.events().recv_timeout(Duration::from_millis(100)) {
            Ok(SeratoRemoteEvent::PeerConnected(_)) => connected = true,
            Ok(SeratoRemoteEvent::Paired(peer)) => {
                paired = true;
                assert_eq!(peer.peer_name.as_deref(), Some("Synthetic Serato"));
            }
            Ok(SeratoRemoteEvent::DeckChange(payload)) => {
                assert!(connected && paired);
                assert_eq!(
                    payload.track.unwrap().artist.as_deref(),
                    Some("Settled Artist")
                );
                changes += 1;
                break;
            }
            _ => {}
        }
    }
    assert_eq!(changes, 1);
    assert_eq!(
        client.get_deck_track(1).unwrap().title.as_deref(),
        Some("Settled Song")
    );
    let before = Instant::now();
    client.stop().unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(!client.running());
    assert!(client.get_deck_track(1).is_none());
    assert_eq!(publisher.stops.load(Ordering::Relaxed), 1);
    assert_peer_closed(&mut peer);
    client.start().unwrap();
    client.stop().unwrap();
    assert_eq!(publisher.stops.load(Ordering::Relaxed), 2);
}
#[test]
fn failed_publisher_tears_down_listener_and_retry_does_not_leak_a_native_daemon() {
    let publisher = Arc::new(Publisher {
        fail: true,
        ..Default::default()
    });
    let mut client = SeratoRemoteClient::with_publisher(client_options(), publisher.clone());
    assert_eq!(client.start(), Err(RemoteError::Publication));
    assert!(!client.running());
    let port = publisher.ports.lock().unwrap()[0];
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    assert_eq!(client.start(), Err(RemoteError::Publication));
    assert!(!client.running());
}
#[test]
fn mdns_record_builder_is_inert_and_matches_empty_txt_and_instance_contract() {
    let options = MdnsPublishOptions {
        peer_name: "Synthetic Remote".into(),
        port: 51337,
        host: Some("fixture.local".into()),
        ..Default::default()
    };
    let record = service_info(&options).unwrap();
    assert_eq!(
        record.get_fullname(),
        r"Synthetic Remote @ fixture\.local._SeratoIOSRemote._tcp.local."
    );
    assert_eq!(record.get_hostname(), "fixture.local.");
    assert_eq!(record.get_port(), 51337);
    assert_eq!(record.get_properties().len(), 0);
    assert!(service_info(&MdnsPublishOptions { port: 0, ..options }).is_err());
}
#[test]
fn bounded_client_event_saturation_retires_owner_and_unregisters_injected_publication() {
    let publisher = Arc::new(Publisher::default());
    let mut client = SeratoRemoteClient::with_publisher(client_options(), publisher.clone());
    let ready = client.start().unwrap();
    let mut peer = TcpStream::connect(("127.0.0.1", ready.port)).unwrap();
    peer.set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let frame = frame_osc(&osc(
        "/Status/Deck/Playhead",
        [arg::i(0), arg::f(1.0), arg::f(1.0), arg::f(128.0)],
    ))
    .unwrap();
    for _ in 0..15 {
        if peer.write_all(&frame.repeat(100)).is_err() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    wait(|| !client.running());
    assert_eq!(client.last_error(), Some(RemoteError::ResourceLimit));
    assert_eq!(publisher.stops.load(Ordering::Relaxed), 1);
    client.stop().unwrap();
    let _ = peer.shutdown(Shutdown::Both);
}

#[test]
fn numeric_loopback_aliases_bind_without_resolver_or_dns_activity() {
    use std::net::{IpAddr, Ipv4Addr};
    let lookups = Arc::new(AtomicUsize::new(0));
    let count = lookups.clone();
    let resolver = Arc::new(BoundedResolver::with_lookup(Arc::new(move |_, _| {
        count.fetch_add(1, Ordering::Relaxed);
        Err(RemoteError::Io)
    })));
    for host in [
        "127.1",
        "127.0.1",
        "127.0.0.1",
        "2130706433",
        "0x7f000001",
        "0177.1",
    ] {
        assert_eq!(
            numeric_host_address(host),
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
        let mut server = RemoteServer::with_resolver(
            RemoteServerOptions {
                host: host.into(),
                ..Default::default()
            },
            resolver.clone(),
        );
        let port = server.start().unwrap();
        let mut peer = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        peer.set_read_timeout(Some(Duration::from_millis(40)))
            .unwrap();
        server.stop();
        assert_peer_closed(&mut peer);
    }
    for host in [
        "999.1",
        "127..1",
        "0x7fzz",
        "127.1.",
        "localhost",
        "synthetic.example.test",
    ] {
        assert!(numeric_host_address(host).is_none());
    }
    assert_eq!(lookups.load(Ordering::Relaxed), 0);
    resolver
        .shutdown(Instant::now() + Duration::from_secs(1))
        .unwrap();
}

#[test]
fn injected_hostname_resolution_preserves_success_failure_and_address_bounds() {
    use std::net::{Ipv4Addr, SocketAddr};
    let queries = Arc::new(Mutex::new(Vec::new()));
    let observed = queries.clone();
    let resolver = Arc::new(BoundedResolver::with_lookup(Arc::new(move |host, port| {
        observed.lock().unwrap().push(host.to_owned());
        match host {
            "localhost" | "synthetic.example.test" => {
                Ok(vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
            }
            "too-many.example.test" => Ok(vec![
                SocketAddr::from((Ipv4Addr::LOCALHOST, port));
                MAX_RESOLVED_ADDRESSES + 1
            ]),
            _ => Err(RemoteError::Io),
        }
    })));
    for host in ["localhost", "synthetic.example.test"] {
        let mut server = RemoteServer::with_resolver(
            RemoteServerOptions {
                host: host.into(),
                ..Default::default()
            },
            resolver.clone(),
        );
        assert!(!server.running());
        let port = server.start().unwrap();
        assert!(port > 0);
        server.stop();
        assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err());
    }
    for (host, error) in [
        ("missing.example.test", RemoteError::Io),
        ("too-many.example.test", RemoteError::ResourceLimit),
    ] {
        let mut server = RemoteServer::with_resolver(
            RemoteServerOptions {
                host: host.into(),
                ..Default::default()
            },
            resolver.clone(),
        );
        assert_eq!(server.start(), Err(error));
        assert!(!server.running());
        assert!(server.port().is_err());
    }
    assert_eq!(queries.lock().unwrap().len(), 4);
    resolver
        .shutdown(Instant::now() + Duration::from_secs(1))
        .unwrap();
}

#[test]
fn resolver_timeouts_reuse_one_worker_skip_stale_queue_and_never_bind_a_late_listener() {
    use std::{
        net::{Ipv4Addr, SocketAddr},
        sync::Condvar,
    };
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0 .0.lock().unwrap_or_else(|e| e.into_inner()) = true;
            self.0 .1.notify_all();
        }
    }
    let _release = Release(gate.clone());
    let blocked = gate.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let resolver = Arc::new(BoundedResolver::with_lookup(Arc::new(move |_, port| {
        counter.fetch_add(1, Ordering::Relaxed);
        let (lock, wake) = &*blocked;
        let ready = lock.lock().unwrap();
        drop(wake.wait_while(ready, |value| !*value).unwrap());
        Ok(vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
    })));
    let mut server = RemoteServer::with_resolver(
        RemoteServerOptions {
            host: "delayed.example.test".into(),
            ..Default::default()
        },
        resolver.clone(),
    );
    let before = Instant::now();
    assert_eq!(server.start(), Err(RemoteError::Deadline));
    assert!(before.elapsed() < Duration::from_secs(2));
    assert!(!server.running());
    assert!(server.port().is_err());
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    // The blocked worker is reused; repeated expired requests cannot spawn
    // detached per-attempt threads or perform their lookups later.
    for _ in 0..RESOLVER_QUEUE_CAPACITY {
        assert_eq!(
            resolver.resolve(
                "queued.example.test",
                0,
                Instant::now() + Duration::from_millis(10)
            ),
            Err(RemoteError::Deadline)
        );
    }
    assert_eq!(
        resolver.resolve(
            "queue-full.example.test",
            0,
            Instant::now() + Duration::from_secs(1)
        ),
        Err(RemoteError::ResourceLimit)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    // An explicit retry may now start. The timed-out original did not leave a
    // listener or ownership behind, and stale queued names are never looked up.
    let retry_deadline = Instant::now() + Duration::from_secs(2);
    let port = loop {
        match server.start() {
            Ok(port) => break port,
            Err(RemoteError::ResourceLimit) if Instant::now() < retry_deadline => {
                thread::sleep(Duration::from_millis(1))
            }
            result => panic!("Released resolver did not recover: {result:?}"),
        }
    };
    assert!(port > 0);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    server.stop();
    assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err());
    resolver
        .shutdown(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        resolver.resolve(
            "closed.example.test",
            0,
            Instant::now() + Duration::from_secs(1)
        ),
        Err(RemoteError::Stopped)
    );
}

#[test]
fn resolver_shutdown_reports_blocked_lookup_then_joins_after_fixture_release() {
    use std::{
        net::{Ipv4Addr, SocketAddr},
        sync::Condvar,
    };
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0 .0.lock().unwrap_or_else(|e| e.into_inner()) = true;
            self.0 .1.notify_all();
        }
    }
    let _release = Release(gate.clone());
    let blocked = gate.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let resolver = BoundedResolver::with_lookup(Arc::new(move |_, port| {
        count.fetch_add(1, Ordering::Relaxed);
        let (lock, wake) = &*blocked;
        drop(
            wake.wait_while(lock.lock().unwrap(), |value| !*value)
                .unwrap(),
        );
        Ok(vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
    }));
    assert_eq!(
        resolver.resolve(
            "blocked.example.test",
            0,
            Instant::now() + Duration::from_millis(500)
        ),
        Err(RemoteError::Deadline)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        resolver.shutdown(Instant::now() + Duration::from_millis(20)),
        Err(RemoteError::Deadline)
    );
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    resolver
        .shutdown(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        resolver.resolve("127.0.0.1", 0, Instant::now() + Duration::from_secs(1)),
        Err(RemoteError::Stopped)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}
