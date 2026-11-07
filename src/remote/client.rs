//! High-level Remote state plus a bounded native listener/publisher facade.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
pub const NUM_REMOTE_DECKS: usize = 4;
pub const DEFAULT_SUBSCRIPTION_TOPICS: [&str; 10] = [
    "/Register/Status/Deck/Playhead",
    "/Register/Status/Deck/Song/Title",
    "/Register/Status/Deck/Song/Artist",
    "/Register/Status/Deck/Song/Filepath",
    "/Register/Status/Deck/Song/Valid",
    "/Register/Status/Deck/Loop/AutoLoopOn",
    "/Register/Status/Deck/Loop/BeatLength",
    "/Register/Status/Deck/Loop/LoopRollOn",
    "/Register/Status/Video/Deck/Mixer/Upfader",
    "/Register/Status/Video/Mixer/Crossfader",
];
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoRemoteTrack {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoRemotePlayhead {
    pub position_seconds: f64,
    pub play_rate: f64,
    pub bpm: f64,
    pub raw: [f64; 3],
}
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoRemoteLoopState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_loop_on: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beat_length: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_roll_on: Option<bool>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoRemoteMixerState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crossfader: Option<f64>,
    pub upfaders: [Option<f64>; 5],
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoRemoteDeckChangePayload {
    pub deck_id: u32,
    pub track: Option<SeratoRemoteTrack>,
    pub previous_track: Option<SeratoRemoteTrack>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoRemotePlayheadPayload {
    pub deck_id: u32,
    pub playhead: SeratoRemotePlayhead,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoRemoteLoopPayload {
    pub deck_id: u32,
    pub r#loop: SeratoRemoteLoopState,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SeratoRemoteMixerPayload {
    pub mixer: SeratoRemoteMixerState,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoRemoteReadyInfo {
    pub port: u16,
    pub instance_name: String,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoRemotePeerInfo {
    pub remote_address: String,
    pub remote_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_uuid: Option<String>,
}
impl From<RemoteSessionPeer> for SeratoRemotePeerInfo {
    fn from(peer: RemoteSessionPeer) -> Self {
        Self {
            remote_address: peer.remote_address,
            remote_port: peer.remote_port,
            peer_name: peer.name,
            peer_uuid: peer.uuid,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum SeratoRemoteEvent {
    Ready(SeratoRemoteReadyInfo),
    PeerConnected(SeratoRemotePeerInfo),
    PeerDisconnected(SeratoRemotePeerInfo),
    Paired(SeratoRemotePeerInfo),
    DeckChange(SeratoRemoteDeckChangePayload),
    Playhead(SeratoRemotePlayheadPayload),
    LoopChange(SeratoRemoteLoopPayload),
    MixerChange(SeratoRemoteMixerPayload),
    Ping,
    Error(RemoteError),
}
pub type SeratoRemoteEvents = SeratoRemoteEvent;
#[derive(Clone)]
pub struct SeratoRemoteOptions {
    pub peer_name: String,
    pub port: u16,
    pub host: String,
    pub subscribe_topics: Vec<String>,
    pub max_frame_bytes: usize,
    pub max_peers: usize,
    pub handshake_timeout: Duration,
    pub idle_timeout: Duration,
    pub mdns_host: Option<String>,
    pub logger: Arc<dyn crate::Logger>,
}
impl Default for SeratoRemoteOptions {
    fn default() -> Self {
        Self {
            peer_name: "serato-connect".into(),
            port: 0,
            host: "0.0.0.0".into(),
            subscribe_topics: DEFAULT_SUBSCRIPTION_TOPICS
                .iter()
                .map(|s| (*s).into())
                .collect(),
            max_frame_bytes: 1 << 20,
            max_peers: 32,
            handshake_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(60),
            mdns_host: None,
            logger: crate::noop_logger(),
        }
    }
}
#[derive(Clone)]
pub struct RemoteState {
    tracks: [Option<SeratoRemoteTrack>; 5],
    emitted: [Option<SeratoRemoteTrack>; 5],
    settle: [Option<u64>; 5],
    loops: [SeratoRemoteLoopState; 5],
    mixer: SeratoRemoteMixerState,
}
impl Default for RemoteState {
    fn default() -> Self {
        Self {
            tracks: std::array::from_fn(|_| None),
            emitted: std::array::from_fn(|_| None),
            settle: [None; 5],
            loops: std::array::from_fn(|_| SeratoRemoteLoopState::default()),
            mixer: Default::default(),
        }
    }
}
impl RemoteState {
    pub fn get_deck_track(&self, deck: u32) -> Option<SeratoRemoteTrack> {
        if !(1..=4).contains(&deck) {
            return None;
        }
        self.tracks[deck as usize].clone()
    }
    pub fn get_deck_loop(&self, deck: u32) -> SeratoRemoteLoopState {
        if !(1..=4).contains(&deck) {
            return Default::default();
        }
        self.loops[deck as usize].clone()
    }
    pub fn get_mixer(&self) -> SeratoRemoteMixerState {
        self.mixer.clone()
    }
    pub fn reset(&mut self) {
        *self = Default::default()
    }
    pub fn flush(&mut self, now: u64) -> Vec<SeratoRemoteEvent> {
        let mut out = Vec::new();
        for deck in 1..=4 {
            if self.settle[deck].is_some_and(|time| time <= now) {
                self.settle[deck] = None;
                self.emit_deck(deck, &mut out)
            }
        }
        out
    }
    fn emit_deck(&mut self, deck: usize, out: &mut Vec<SeratoRemoteEvent>) {
        if self.tracks[deck] != self.emitted[deck] {
            let previous = self.emitted[deck].clone();
            self.emitted[deck] = self.tracks[deck].clone();
            out.push(SeratoRemoteEvent::DeckChange(
                SeratoRemoteDeckChangePayload {
                    deck_id: deck as u32,
                    track: self.tracks[deck].clone(),
                    previous_track: previous,
                },
            ))
        }
    }
    pub fn handle_status(&mut self, message: &OscMessage, now: u64) -> Vec<SeratoRemoteEvent> {
        let mut out = Vec::new();
        let values = &message.args;
        if message.address == "/Status/Video/Mixer/Crossfader" {
            if let Some(value) = float_arg(values, 0) {
                if self.mixer.crossfader != Some(value) {
                    self.mixer.crossfader = Some(value);
                    out.push(SeratoRemoteEvent::MixerChange(SeratoRemoteMixerPayload {
                        mixer: self.mixer.clone(),
                    }));
                }
            }
            return out;
        }
        let Some(raw) = int_arg(values, 0) else {
            return out;
        };
        let deck = if raw == 4 {
            4
        } else if (0..4).contains(&raw) {
            raw as usize + 1
        } else {
            return out;
        };
        match message.address.as_str() {
            "/Status/Deck/Song/Title"
            | "/Status/Deck/Song/Artist"
            | "/Status/Deck/Song/Filepath"
            | "/Status/Deck/Song/Valid" => {
                let mut track = self.tracks[deck].clone().unwrap_or_default();
                match message.address.as_str() {
                    "/Status/Deck/Song/Title" => {
                        track.title = Some(string_arg(values, 1).unwrap_or_default())
                    }
                    "/Status/Deck/Song/Artist" => {
                        track.artist = Some(string_arg(values, 1).unwrap_or_default())
                    }
                    "/Status/Deck/Song/Filepath" => {
                        track.file_path = Some(string_arg(values, 1).unwrap_or_default())
                    }
                    _ => track.valid = bool_arg(values, 1),
                }
                let invalid =
                    message.address == "/Status/Deck/Song/Valid" && track.valid == Some(false);
                if self.tracks[deck].as_ref() != Some(&track) {
                    self.tracks[deck] = Some(track);
                    self.settle[deck] = Some(now.saturating_add(50));
                }
                if invalid {
                    self.settle[deck] = None;
                    self.tracks[deck] = None;
                    self.emit_deck(deck, &mut out)
                }
            }
            "/Status/Deck/Playhead" => {
                if let (Some(a), Some(b), Some(c)) = (
                    float_arg(values, 1),
                    float_arg(values, 2),
                    float_arg(values, 3),
                ) {
                    out.push(SeratoRemoteEvent::Playhead(SeratoRemotePlayheadPayload {
                        deck_id: deck as u32,
                        playhead: SeratoRemotePlayhead {
                            position_seconds: a,
                            play_rate: b,
                            bpm: c,
                            raw: [a, b, c],
                        },
                    }));
                }
            }
            "/Status/Deck/Loop/AutoLoopOn"
            | "/Status/Deck/Loop/BeatLength"
            | "/Status/Deck/Loop/LoopRollOn" => {
                let mut next = self.loops[deck].clone();
                match message.address.as_str() {
                    "/Status/Deck/Loop/AutoLoopOn" => next.auto_loop_on = bool_arg(values, 1),
                    "/Status/Deck/Loop/BeatLength" => next.beat_length = float_arg(values, 1),
                    _ => next.loop_roll_on = bool_arg(values, 1),
                }
                if next != self.loops[deck] {
                    self.loops[deck] = next.clone();
                    out.push(SeratoRemoteEvent::LoopChange(SeratoRemoteLoopPayload {
                        deck_id: deck as u32,
                        r#loop: next,
                    }));
                }
            }
            "/Status/Video/Deck/Mixer/Upfader" => {
                if let Some(value) = float_arg(values, 1) {
                    if self.mixer.upfaders[deck] != Some(value) {
                        self.mixer.upfaders[deck] = Some(value);
                        out.push(SeratoRemoteEvent::MixerChange(SeratoRemoteMixerPayload {
                            mixer: self.mixer.clone(),
                        }));
                    }
                }
            }
            _ => {}
        }
        out
    }
}
fn float_arg(args: &[OscArg], index: usize) -> Option<f64> {
    match args.get(index)? {
        OscArg::Int(v) => Some(f64::from(*v)),
        OscArg::Float(v) => Some(f64::from(*v)),
        _ => None,
    }
}
fn int_arg(args: &[OscArg], index: usize) -> Option<i32> {
    let value = float_arg(args, index)?;
    if !value.is_finite() || value > i32::MAX as f64 || value < i32::MIN as f64 {
        return None;
    }
    Some((value + 0.5).floor() as i32)
}
fn bool_arg(args: &[OscArg], index: usize) -> Option<bool> {
    float_arg(args, index).map(|v| v >= 0.5)
}
fn string_arg(args: &[OscArg], index: usize) -> Option<String> {
    if let Some(OscArg::String(value)) = args.get(index) {
        Some(value.clone())
    } else {
        None
    }
}
pub struct SeratoRemoteClient {
    options: SeratoRemoteOptions,
    publisher: Arc<dyn MdnsPublisher>,
    state: Arc<Mutex<RemoteState>>,
    running: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<RemoteError>>>,
    sender: mpsc::SyncSender<SeratoRemoteEvent>,
    events: Receiver<SeratoRemoteEvent>,
    thread: Option<JoinHandle<()>>,
}
impl SeratoRemoteClient {
    pub fn new(options: SeratoRemoteOptions) -> Self {
        Self::with_publisher(options, Arc::new(NativeMdnsPublisher))
    }
    pub fn with_publisher(options: SeratoRemoteOptions, publisher: Arc<dyn MdnsPublisher>) -> Self {
        let (sender, events) = mpsc::sync_channel(512);
        Self {
            options,
            publisher,
            state: Arc::new(Mutex::new(RemoteState::default())),
            running: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            error: Arc::new(Mutex::new(None)),
            sender,
            events,
            thread: None,
        }
    }
    pub fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
    pub fn last_error(&self) -> Option<RemoteError> {
        *self.error.lock().unwrap()
    }
    pub fn events(&self) -> &Receiver<SeratoRemoteEvent> {
        &self.events
    }
    pub fn get_deck_track(&self, deck: u32) -> Option<SeratoRemoteTrack> {
        self.state.lock().unwrap().get_deck_track(deck)
    }
    pub fn get_deck_loop(&self, deck: u32) -> SeratoRemoteLoopState {
        self.state.lock().unwrap().get_deck_loop(deck)
    }
    pub fn get_mixer(&self) -> SeratoRemoteMixerState {
        self.state.lock().unwrap().get_mixer()
    }
    pub fn start(&mut self) -> RemoteResult<SeratoRemoteReadyInfo> {
        if self.running() {
            return Err(RemoteError::AlreadyRunning);
        }
        if self.thread.is_some() {
            self.stop()?
        }
        while self.events.try_recv().is_ok() {}
        let mut server = RemoteServer::new(RemoteServerOptions {
            port: self.options.port,
            host: self.options.host.clone(),
            peer_name: self.options.peer_name.clone(),
            subscribe_topics: self.options.subscribe_topics.clone(),
            max_frame_bytes: self.options.max_frame_bytes,
            max_peers: self.options.max_peers,
            handshake_timeout: self.options.handshake_timeout,
            idle_timeout: self.options.idle_timeout,
            logger: self.options.logger.clone(),
            ..Default::default()
        });
        let port = server.start()?;
        let mut publication = match self.publisher.publish(&MdnsPublishOptions {
            peer_name: self.options.peer_name.clone(),
            port,
            host: self.options.mdns_host.clone(),
            logger: self.options.logger.clone(),
        }) {
            Ok(publication) => publication,
            Err(error) => {
                server.stop();
                return Err(error);
            }
        };
        let info = SeratoRemoteReadyInfo {
            port,
            instance_name: publication.instance_name().into(),
        };
        self.sender
            .try_send(SeratoRemoteEvent::Ready(info.clone()))
            .map_err(|_| RemoteError::ResourceLimit)?;
        self.running.store(true, Ordering::Release);
        self.stop.store(false, Ordering::Release);
        *self.error.lock().unwrap() = None;
        self.state.lock().unwrap().reset();
        let (state, running, stop, error, sender) = (
            self.state.clone(),
            self.running.clone(),
            self.stop.clone(),
            self.error.clone(),
            self.sender.clone(),
        );
        self.thread = Some(
            thread::Builder::new()
                .name("serato-remote-client".into())
                .spawn(move || {
                    struct Lease {
                        running: Arc<AtomicBool>,
                        state: Arc<Mutex<RemoteState>>,
                        error: Arc<Mutex<Option<RemoteError>>>,
                    }
                    impl Drop for Lease {
                        fn drop(&mut self) {
                            self.running.store(false, Ordering::Release);
                            self.state.lock().unwrap_or_else(|e| e.into_inner()).reset();
                            if thread::panicking() {
                                *self.error.lock().unwrap_or_else(|e| e.into_inner()) =
                                    Some(RemoteError::Io);
                            }
                        }
                    }
                    let _lease = Lease {
                        running: running.clone(),
                        state: state.clone(),
                        error: error.clone(),
                    };
                    let started = Instant::now();
                    let mut peers: BTreeMap<u64, Arc<RemoteSession>> = BTreeMap::new();
                    let result = (|| -> RemoteResult<()> {
                        loop {
                            if stop.load(Ordering::Acquire) {
                                return Ok(());
                            }
                            for _ in 0..256 {
                                let Ok(event) = server.events().try_recv() else {
                                    break;
                                };
                                let output = match event {
                                    RemoteServerEvent::Listening { .. } => Vec::new(),
                                    RemoteServerEvent::Error(error) => {
                                        vec![SeratoRemoteEvent::Error(error)]
                                    }
                                    RemoteServerEvent::Session {
                                        session_id,
                                        session,
                                    } => {
                                        let peer = session.remote_peer().into();
                                        peers.insert(session_id, session);
                                        vec![SeratoRemoteEvent::PeerConnected(peer)]
                                    }
                                    RemoteServerEvent::SessionEvent { session_id, event } => {
                                        match event {
                                            RemoteSessionEvent::Status(message, _) => {
                                                state.lock().unwrap().handle_status(
                                                    &message,
                                                    started
                                                        .elapsed()
                                                        .as_millis()
                                                        .min(u64::MAX as u128)
                                                        as u64,
                                                )
                                            }
                                            RemoteSessionEvent::Paired(peer) => {
                                                vec![SeratoRemoteEvent::Paired(peer.into())]
                                            }
                                            RemoteSessionEvent::Ping(_) => {
                                                vec![SeratoRemoteEvent::Ping]
                                            }
                                            RemoteSessionEvent::Error(error, _) => {
                                                vec![SeratoRemoteEvent::Error(error)]
                                            }
                                            RemoteSessionEvent::Closed(peer) => {
                                                if peers.remove(&session_id).is_some() {
                                                    vec![SeratoRemoteEvent::PeerDisconnected(
                                                        peer.into(),
                                                    )]
                                                } else {
                                                    Vec::new()
                                                }
                                            }
                                        }
                                    }
                                };
                                for event in output {
                                    sender
                                        .try_send(event)
                                        .map_err(|_| RemoteError::ResourceLimit)?;
                                }
                            }
                            let now = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                            for event in state.lock().unwrap().flush(now) {
                                sender
                                    .try_send(event)
                                    .map_err(|_| RemoteError::ResourceLimit)?;
                            }
                            // A saturated transport can lose its terminal channel event;
                            // actual session ownership still authoritatively removes peers.
                            let dead: Vec<_> = peers
                                .iter()
                                .filter(|(_, s)| !s.running())
                                .map(|(id, _)| *id)
                                .collect();
                            for id in dead {
                                if let Some(peer) = peers.remove(&id) {
                                    sender
                                        .try_send(SeratoRemoteEvent::PeerDisconnected(
                                            peer.remote_peer().into(),
                                        ))
                                        .map_err(|_| RemoteError::ResourceLimit)?;
                                }
                            }
                            if !server.running() {
                                return Err(RemoteError::Io);
                            }
                            thread::sleep(Duration::from_millis(5));
                        }
                    })();
                    if let Err(failure) = result {
                        *error.lock().unwrap() = Some(failure);
                        let _ = sender.try_send(SeratoRemoteEvent::Error(failure));
                    }
                    server.stop();
                    if publication.stop().is_err() {
                        *error.lock().unwrap() = Some(RemoteError::Publication)
                    }
                    state.lock().unwrap().reset();
                    running.store(false, Ordering::Release);
                })
                .map_err(|_| {
                    self.running.store(false, Ordering::Release);
                    RemoteError::Io
                })?,
        );
        Ok(info)
    }
    pub fn stop(&mut self) -> RemoteResult<()> {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
        self.running.store(false, Ordering::Release);
        self.state.lock().unwrap().reset();
        while self.events.try_recv().is_ok() {}
        Ok(())
    }
}
impl Drop for SeratoRemoteClient {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub type SeratoRemoteEmitter = Receiver<SeratoRemoteEvent>;
