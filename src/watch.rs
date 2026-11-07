//! Single-owner history polling. The initial history seeds deck state without
//! replaying old history; every later poll is serialized with the preceding one.
use crate::{
    history_v3::*, history_v4::*, noop_logger, Error, Logger, Result, SeratoHistorySong,
    SeratoSession, SeratoVersion,
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
#[derive(Clone)]
pub struct SeratoConnectOptions {
    pub serato_path: Option<PathBuf>,
    pub library_path: Option<PathBuf>,
    pub force_version: Option<SeratoVersion>,
    pub poll_interval: Duration,
    pub history_max_rows: usize,
    pub discovery: Option<DiscoveryContext>,
    pub logger: Arc<dyn Logger>,
}
impl Default for SeratoConnectOptions {
    fn default() -> Self {
        Self {
            serato_path: None,
            library_path: None,
            force_version: None,
            poll_interval: Duration::from_secs(2),
            history_max_rows: 100,
            discovery: None,
            logger: noop_logger(),
        }
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoReadyInfo {
    pub serato_path: PathBuf,
    pub session_count: usize,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoTrackPayload {
    pub track: SeratoHistorySong,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deck_id: Option<u32>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoHistoryPayload {
    pub serato_path: PathBuf,
    pub count: usize,
    pub tracks: Vec<SeratoHistorySong>,
    pub last_track_index: usize,
}
#[derive(Clone, Debug, Serialize)]
pub struct SeratoSessionPayload {
    pub session: SeratoSession,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoDeckChangePayload {
    pub deck_id: u32,
    pub track: Option<SeratoHistorySong>,
    pub previous_track: Option<SeratoHistorySong>,
}
#[derive(Clone, Debug)]
pub enum SeratoEvent {
    Ready(SeratoReadyInfo),
    Poll,
    DeckChange(Box<SeratoDeckChangePayload>),
    Track(Box<SeratoTrackPayload>),
    History(SeratoHistoryPayload),
    Session(SeratoSessionPayload),
    Error(Error),
}
pub type SeratoConnectEvents = SeratoEvent;
#[derive(Clone)]
pub struct Installation {
    pub found: bool,
    pub path: PathBuf,
    pub has_history: bool,
    pub version: SeratoVersion,
}
pub fn get_default_serato_path() -> Result<PathBuf> {
    DiscoveryContext::current()?.v3_path()
}
pub fn detect_serato_version(explicit: Option<&Path>) -> Result<SeratoVersion> {
    detect_serato_version_in(explicit, &DiscoveryContext::current()?)
}
pub fn detect_serato_version_in(
    explicit: Option<&Path>,
    context: &DiscoveryContext,
) -> Result<SeratoVersion> {
    if let Some(path) = explicit {
        return Ok(if path.exists() && path.join("History").exists() {
            SeratoVersion::V3
        } else {
            SeratoVersion::Unknown
        });
    }
    let v3 = context.v3_path()?;
    let v4 = context.v4_path()?;
    let old = v3.join("History").exists();
    let new = has_serato_v4_database(Some(&v4))?;
    Ok(match (old, new) {
        (true, true) => {
            let old = std::fs::metadata(v3.join("History"))?.modified()?;
            let new = std::fs::metadata(v4.join("master.sqlite"))?.modified()?;
            if old > new {
                SeratoVersion::V3
            } else {
                SeratoVersion::V4
            }
        }
        (true, false) => SeratoVersion::V3,
        (false, true) => SeratoVersion::V4,
        _ => SeratoVersion::Unknown,
    })
}
pub fn detect_serato_installation(
    explicit: Option<&Path>,
    additional: &[PathBuf],
) -> Result<Installation> {
    detect_serato_installation_in(explicit, additional, &DiscoveryContext::current()?)
}
pub fn detect_serato_installation_in(
    explicit: Option<&Path>,
    additional: &[PathBuf],
    context: &DiscoveryContext,
) -> Result<Installation> {
    if let Some(path) = explicit {
        let found = path.exists();
        let has_history = found && path.join("History").exists();
        return Ok(Installation {
            found,
            path: path.to_owned(),
            has_history,
            version: if has_history {
                SeratoVersion::V3
            } else {
                SeratoVersion::Unknown
            },
        });
    }
    if detect_serato_version_in(None, context)? == SeratoVersion::V4 {
        return Ok(Installation {
            found: true,
            path: context.v4_path()?,
            has_history: true,
            version: SeratoVersion::V4,
        });
    }
    let default = context.v3_path()?;
    for path in std::iter::once(&default).chain(additional) {
        if path.join("History").exists() {
            return Ok(Installation {
                found: true,
                path: path.clone(),
                has_history: true,
                version: SeratoVersion::V3,
            });
        }
    }
    Ok(Installation {
        found: false,
        path: default,
        has_history: false,
        version: SeratoVersion::Unknown,
    })
}
#[derive(Clone, Default)]
struct Snapshot {
    version: SeratoVersion,
    decks: [Option<SeratoHistorySong>; 4],
    error: Option<Error>,
}
pub struct SeratoConnect {
    options: SeratoConnectOptions,
    snapshot: Arc<Mutex<Snapshot>>,
    running: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    control: Arc<(Mutex<(bool, Duration)>, Condvar)>,
    sender: SyncSender<SeratoEvent>,
    receiver: Receiver<SeratoEvent>,
    thread: Option<JoinHandle<()>>,
}
impl SeratoConnect {
    pub fn new(options: SeratoConnectOptions) -> Self {
        let (sender, receiver) = mpsc::sync_channel(256);
        let interval = options.poll_interval;
        Self {
            options,
            snapshot: Arc::new(Mutex::new(Snapshot::default())),
            running: Arc::new(AtomicBool::new(false)),
            cancelled: Arc::new(AtomicBool::new(false)),
            control: Arc::new((Mutex::new((false, interval)), Condvar::new())),
            sender,
            receiver,
            thread: None,
        }
    }
    pub fn events(&self) -> &Receiver<SeratoEvent> {
        &self.receiver
    }
    pub fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
    pub fn version(&self) -> SeratoVersion {
        self.snapshot.lock().unwrap().version
    }
    pub fn last_error(&self) -> Option<Error> {
        self.snapshot.lock().unwrap().error
    }
    pub fn get_deck_track(&self, deck: u32) -> Option<SeratoHistorySong> {
        self.snapshot
            .lock()
            .unwrap()
            .decks
            .get(deck.checked_sub(1)? as usize)
            .cloned()
            .flatten()
    }
    pub fn get_all_deck_states(&self) -> [Option<SeratoHistorySong>; 4] {
        self.snapshot.lock().unwrap().decks.clone()
    }
    pub fn get_now_playing(&self) -> Option<SeratoHistorySong> {
        let snapshot = self.snapshot.lock().unwrap();
        snapshot
            .decks
            .iter()
            .enumerate()
            .filter_map(|(i, song)| song.as_ref().map(|song| (i, song)))
            .max_by_key(|(i, song)| (song.start_time.unwrap_or(0), std::cmp::Reverse(*i)))
            .map(|(_, song)| song.clone())
    }
    pub fn poll_interval(&self) -> Duration {
        self.control.0.lock().unwrap().1
    }
    pub fn set_poll_interval(&mut self, interval: Duration) -> Result<()> {
        if interval.is_zero() || interval > Duration::from_secs(86400) {
            return Err(Error::InvalidData);
        }
        self.options.poll_interval = interval;
        self.control.0.lock().unwrap().1 = interval;
        self.control.1.notify_all();
        Ok(())
    }
    pub fn start(&mut self) -> Result<()> {
        if self.running() {
            return Ok(());
        }
        if self.thread.is_some() {
            self.stop()
        }
        while self.receiver.try_recv().is_ok() {}
        let result = self.begin();
        if let Err(error) = result {
            self.snapshot.lock().unwrap().error = Some(error);
            let _ = self.sender.try_send(SeratoEvent::Error(error));
        }
        result
    }
    fn begin(&mut self) -> Result<()> {
        if self.options.poll_interval.is_zero()
            || self.options.poll_interval > Duration::from_secs(86400)
            || self.options.history_max_rows > 1_000_000
        {
            return Err(Error::InvalidData);
        }
        let context = match &self.options.discovery {
            Some(context) => context.clone(),
            None => DiscoveryContext::current()?,
        };
        let version = match self.options.force_version {
            Some(version) => version,
            None => {
                if self.options.library_path.is_some() {
                    SeratoVersion::V4
                } else {
                    detect_serato_version_in(self.options.serato_path.as_deref(), &context)?
                }
            }
        };
        let path = match version {
            SeratoVersion::V3 => self
                .options
                .serato_path
                .clone()
                .map(Ok)
                .unwrap_or_else(|| context.v3_path())?,
            SeratoVersion::V4 => self
                .options
                .library_path
                .clone()
                .or_else(|| self.options.serato_path.clone())
                .map(Ok)
                .unwrap_or_else(|| context.v4_path())?,
            SeratoVersion::Unknown => return Err(Error::InvalidData),
        };
        let mut poller = Poller::new(version, path, self.options.history_max_rows)?;
        let decks = compute_deck_states(&poller.songs);
        self.sender
            .try_send(SeratoEvent::Ready(SeratoReadyInfo {
                serato_path: poller.path.clone(),
                session_count: poller.session_count,
            }))
            .map_err(|_| Error::ResourceLimit)?;
        for (i, track) in decks.iter().enumerate() {
            if track.is_some() {
                self.sender
                    .try_send(SeratoEvent::DeckChange(Box::new(SeratoDeckChangePayload {
                        deck_id: i as u32 + 1,
                        track: track.clone(),
                        previous_track: None,
                    })))
                    .map_err(|_| Error::ResourceLimit)?;
            }
        }
        *self.snapshot.lock().unwrap() = Snapshot {
            version,
            decks,
            error: None,
        };
        poller.cursor = poller.songs.len();
        self.control.0.lock().unwrap().0 = false;
        self.cancelled.store(false, Ordering::Release);
        self.running.store(true, Ordering::Release);
        let cancelled = self.cancelled.clone();
        let (snapshot, running, control, sender) = (
            self.snapshot.clone(),
            self.running.clone(),
            self.control.clone(),
            self.sender.clone(),
        );
        self.thread = Some(
            thread::Builder::new()
                .name("serato-history".into())
                .spawn(move || {
                    struct Lease(Arc<AtomicBool>);
                    impl Drop for Lease {
                        fn drop(&mut self) {
                            self.0.store(false, Ordering::Release)
                        }
                    }
                    let _lease = Lease(running);
                    loop {
                        let guard = control.0.lock().unwrap_or_else(|e| e.into_inner());
                        let interval = guard.1;
                        let (guard, _) = control
                            .1
                            .wait_timeout(guard, interval)
                            .unwrap_or_else(|e| e.into_inner());
                        if guard.0 {
                            break;
                        }
                        drop(guard);
                        if sender.try_send(SeratoEvent::Poll).is_err() {
                            snapshot.lock().unwrap().error = Some(Error::ResourceLimit);
                            break;
                        }
                        match poller.poll(&snapshot, &cancelled) {
                            Ok(events) => {
                                let mut full = false;
                                for event in events {
                                    if sender.try_send(event).is_err() {
                                        full = true;
                                        break;
                                    }
                                }
                                if full {
                                    snapshot.lock().unwrap().error = Some(Error::ResourceLimit);
                                    break;
                                }
                            }
                            Err(error) => {
                                snapshot.lock().unwrap().error = Some(error);
                                if sender.try_send(SeratoEvent::Error(error)).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                })
                .map_err(|_| {
                    self.running.store(false, Ordering::Release);
                    Error::ThreadFailed
                })?,
        );
        Ok(())
    }
    fn library_root(&self) -> Result<PathBuf> {
        // History ownership may switch to a V4 SQLite library, but crate and
        // database-V2 APIs still address the independent legacy _Serato_ root.
        // Never derive metadata location from the currently active history
        // snapshot: doing so silently empties these queries after V4 start.
        if let Some(path) = &self.options.serato_path {
            return Ok(path.clone());
        }
        match &self.options.discovery {
            Some(context) => context.v3_path(),
            None => get_default_serato_path(),
        }
    }
    pub fn get_track_metadata(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<Option<crate::SeratoTrackMetadata>> {
        crate::get_track_metadata(path)
    }
    pub fn get_crates(&self) -> Result<Vec<crate::SeratoCrate>> {
        crate::get_all_crates(self.library_root()?)
    }
    pub fn get_crate(&self, name: &str) -> Result<Option<crate::SeratoCrate>> {
        let suffix = format!("{name}.crate");
        for path in crate::list_crates(self.library_root()?)? {
            if path.ends_with(&suffix) {
                return crate::parse_crate(path).map(Some);
            }
        }
        Ok(None)
    }
    pub fn find_crates_for_track(&self, path: &str) -> Result<Vec<String>> {
        crate::find_crates_for_track(self.library_root()?, path)
    }
    pub fn get_library_tracks(
        &self,
        query: Option<LibraryQuery>,
    ) -> Result<Vec<crate::SeratoDatabaseTrack>> {
        let tracks = crate::get_library_tracks(self.library_root()?)?;
        let Some(query) = query else {
            return Ok(tracks);
        };
        let offset = query.offset.unwrap_or(0);
        let end = query
            .limit
            .filter(|v| *v != 0)
            .map(|limit| offset.saturating_add(limit))
            .unwrap_or(tracks.len() as i64);
        let normalize = |value: i64| {
            if value < 0 {
                (tracks.len() as i64).saturating_add(value).max(0) as usize
            } else {
                (value as usize).min(tracks.len())
            }
        };
        let start = normalize(offset);
        let end = normalize(end).max(start);
        Ok(tracks[start..end].to_vec())
    }
    pub fn get_track_by_path(&self, path: &str) -> Result<Option<crate::SeratoDatabaseTrack>> {
        crate::get_track_by_path(self.library_root()?, path)
    }
    pub fn search_library(&self, query: &str) -> Result<Vec<crate::SeratoDatabaseTrack>> {
        crate::search_library(self.library_root()?, query)
    }
    pub fn get_database_stats(&self) -> Result<crate::SeratoDatabaseStats> {
        crate::get_database_stats(self.library_root()?)
    }
    pub fn stop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.control.0.lock().unwrap().0 = true;
        self.control.1.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.running.store(false, Ordering::Release);
        self.snapshot.lock().unwrap().decks = Default::default();
        while self.receiver.try_recv().is_ok() {}
    }
}
impl Drop for SeratoConnect {
    fn drop(&mut self) {
        self.stop()
    }
}
struct Poller {
    version: SeratoVersion,
    path: PathBuf,
    session_id: Option<i64>,
    session_file: Option<PathBuf>,
    mtime: Option<std::time::SystemTime>,
    songs: Vec<SeratoHistorySong>,
    cursor: usize,
    max: usize,
    session_count: usize,
}
impl Poller {
    fn new(version: SeratoVersion, path: PathBuf, max: usize) -> Result<Self> {
        let (mut session_id, mut session_file, mut mtime) = (None, None, None);
        let (session_count, songs) = if version == SeratoVersion::V4 {
            if !has_serato_v4_database(Some(&path))? {
                return Err(Error::Io);
            }
            let sessions = get_serato_v4_sessions(Some(&path))?;
            let latest = latest_snapshot_v4(&path, None)?;
            session_id = latest.as_ref().map(|(info, _)| info.id);
            (
                sessions.len(),
                latest.map(|(_, rows)| rows).unwrap_or_default(),
            )
        } else {
            if !path.join("History").exists() {
                return Err(Error::Io);
            }
            let sessions = get_serato_history(&path)?;
            session_file = get_latest_session_path(&path)?;
            let songs = if let Some(file) = &session_file {
                mtime = Some(std::fs::metadata(file)?.modified()?);
                get_session_songs(file)?
            } else {
                Vec::new()
            };
            (sessions.len(), songs)
        };
        Ok(Self {
            version,
            path,
            session_id,
            session_file,
            mtime,
            songs,
            cursor: 0,
            max,
            session_count,
        })
    }
    fn poll(
        &mut self,
        snapshot: &Mutex<Snapshot>,
        cancelled: &AtomicBool,
    ) -> Result<Vec<SeratoEvent>> {
        let is_new = if self.version == SeratoVersion::V4 {
            let Some((info, songs)) = latest_snapshot_v4(&self.path, Some(cancelled))? else {
                return Ok(Vec::new());
            };
            let changed = self.session_id != Some(info.id);
            self.session_id = Some(info.id);
            // Count-only checks miss in-place played/end-time updates.
            self.songs = songs;
            changed
        } else {
            let Some(file) = get_latest_session_path(&self.path)? else {
                return Ok(Vec::new());
            };
            let mtime = std::fs::metadata(&file)?.modified()?;
            if self.session_file.as_ref() == Some(&file) && self.mtime == Some(mtime) {
                return Ok(Vec::new());
            }
            let changed = self.session_file.as_ref() != Some(&file);
            let songs = get_session_songs(&file)?;
            self.session_file = Some(file);
            self.mtime = Some(mtime);
            self.songs = songs;
            changed
        };
        let mut out = Vec::new();
        let mut snapshot = snapshot.lock().unwrap();
        if is_new {
            self.cursor = 0;
            for (i, old) in snapshot.decks.iter().enumerate() {
                if old.is_some() {
                    out.push(SeratoEvent::DeckChange(Box::new(SeratoDeckChangePayload {
                        deck_id: i as u32 + 1,
                        track: None,
                        previous_track: old.clone(),
                    })))
                }
            }
            snapshot.decks = Default::default();
        }
        let decks = compute_deck_states(&self.songs);
        for (i, track) in decks.iter().enumerate() {
            if track_key(track.as_ref()) != track_key(snapshot.decks[i].as_ref()) {
                out.push(SeratoEvent::DeckChange(Box::new(SeratoDeckChangePayload {
                    deck_id: i as u32 + 1,
                    track: track.clone(),
                    previous_track: snapshot.decks[i].clone(),
                })))
            }
        }
        snapshot.decks = decks;
        snapshot.error = None;
        if self.songs.len() < self.cursor {
            self.cursor = self.songs.len();
        }
        if self.songs.len() > self.cursor {
            let new = &self.songs[self.cursor..];
            let count = new.len();
            let tracks = if self.max == 0 {
                new.to_vec()
            } else {
                new[new.len().saturating_sub(self.max)..].to_vec()
            };
            self.cursor = self.songs.len();
            out.push(SeratoEvent::History(SeratoHistoryPayload {
                serato_path: self.path.clone(),
                count,
                tracks,
                last_track_index: self.cursor,
            }));
        }
        if is_new {
            let index = if self.version == SeratoVersion::V4 {
                self.session_id.unwrap_or(0)
            } else {
                self.session_file
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0)
            };
            out.push(SeratoEvent::Session(SeratoSessionPayload {
                session: SeratoSession {
                    date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
                    index,
                    songs: self.songs.clone(),
                },
            }));
        }
        Ok(out)
    }
}
pub fn compute_deck_states(songs: &[SeratoHistorySong]) -> [Option<SeratoHistorySong>; 4] {
    let mut decks = std::array::from_fn(|_| None);
    let mut assigned = false;
    for song in songs {
        if let Some(deck) = song.deck.filter(|d| (1..=4).contains(d)) {
            assigned = true;
            let index = deck as usize - 1;
            if song.start_time.is_some() && song.play_time.is_none() {
                decks[index] = Some(song.clone())
            } else if song.play_time.is_some() {
                decks[index] = None
            }
        }
    }
    if !assigned {
        for (slot, song) in songs
            .iter()
            .filter(|s| s.start_time.is_some() && s.play_time.is_none())
            .take(4)
            .enumerate()
        {
            decks[slot] = Some(song.clone())
        }
    }
    decks
}
fn track_key(song: Option<&SeratoHistorySong>) -> Option<String> {
    song.map(|s| {
        format!(
            "{}|{}|{}|{}",
            s.artist,
            s.title,
            s.start_time.unwrap_or(0),
            u8::from(s.played == Some(true))
        )
    })
}

#[derive(Clone, Copy, Default)]
pub struct LibraryQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
