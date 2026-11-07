//! Plaintext Serato 4 SQLite helpers. Read-only connections are short-lived;
//! callers explicitly provide library paths when processing fixtures.
use crate::{Error, Result, SeratoHistorySong, SeratoSession};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Unsupported,
}
impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unsupported
        }
    }
}
#[derive(Clone)]
pub struct DiscoveryContext {
    pub platform: Platform,
    pub home: PathBuf,
    pub local_app_data: Option<PathBuf>,
    pub music_directory: Option<PathBuf>,
}
impl DiscoveryContext {
    pub fn current() -> Result<Self> {
        let home = if cfg!(windows) {
            std::env::var_os("USERPROFILE")
        } else {
            std::env::var_os("HOME")
        }
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or(Error::UnsupportedPlatform)?;
        Ok(Self {
            platform: Platform::current(),
            home,
            local_app_data: std::env::var_os("LOCALAPPDATA")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            music_directory: None,
        })
    }
    pub fn v3_path(&self) -> Result<PathBuf> {
        if self.platform == Platform::Unsupported {
            return Err(Error::UnsupportedPlatform);
        }
        Ok(self
            .music_directory
            .clone()
            .unwrap_or_else(|| self.home.join("Music"))
            .join("_Serato_"))
    }
    pub fn v4_path(&self) -> Result<PathBuf> {
        match self.platform {
            Platform::MacOs => Ok(self.home.join("Library/Application Support/Serato/Library")),
            Platform::Windows => Ok(self
                .local_app_data
                .clone()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| self.home.join("AppData/Local"))
                .join("Serato/Library")),
            Platform::Unsupported => Err(Error::UnsupportedPlatform),
        }
    }
}
pub fn get_default_serato_v4_library_path() -> Result<PathBuf> {
    DiscoveryContext::current()?.v4_path()
}
pub fn get_master_database_path(path: Option<&Path>) -> Result<PathBuf> {
    Ok(match path {
        Some(path) => path.to_owned(),
        None => get_default_serato_v4_library_path()?,
    }
    .join("master.sqlite"))
}
pub fn has_serato_v4_database(path: Option<&Path>) -> Result<bool> {
    Ok(get_master_database_path(path)?.exists())
}
fn connection(path: Option<&Path>) -> Result<Option<Connection>> {
    let path = get_master_database_path(path)?;
    if !path.exists() {
        return Ok(None);
    }
    // SQLite's own native open is blocking. Reject known FIFOs/devices using
    // the shared nonblocking regular-file guard before handing it the path.
    // Keep that verified handle alive through the SQLite open. This does not
    // claim protection against a concurrent hostile pathname replacement.
    let _regular = crate::util::open_regular(&path)?;
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| Error::Io)?;
    db.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|_| Error::Io)?;
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 16 << 20)
        .map_err(|_| Error::Io)?;
    Ok(Some(db))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestSessionInfo {
    pub id: i64,
    pub entry_count: usize,
}
pub fn get_latest_session_info_v4(path: Option<&Path>) -> Result<Option<LatestSessionInfo>> {
    let Some(db) = connection(path)? else {
        return Ok(None);
    };
    let mut query = db
        .prepare("SELECT id FROM history_session ORDER BY start_time DESC LIMIT 1")
        .map_err(|_| Error::InvalidData)?;
    let mut rows = query.query([]).map_err(|_| Error::InvalidData)?;
    let Some(row) = rows.next().map_err(|_| Error::InvalidData)? else {
        return Ok(None);
    };
    let id = row.get::<_, i64>(0).map_err(|_| Error::InvalidData)?;
    let count = db
        .query_row(
            "SELECT COUNT(*) FROM history_entry WHERE session_id=?",
            [id],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|_| Error::InvalidData)?;
    Ok(Some(LatestSessionInfo {
        id,
        entry_count: usize::try_from(count).map_err(|_| Error::ResourceLimit)?,
    }))
}
const MAX_MAPPING_ROWS: usize = 1_000_000;
const MAX_MAPPING_URI_BYTES: usize = 16 * 1024 * 1024;
fn mappings(
    db: &Connection,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> Result<std::collections::BTreeMap<i64, String>> {
    let mut out = std::collections::BTreeMap::new();
    if cancelled.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
        return Err(Error::Stopped);
    }
    let mut query = match db.prepare("SELECT location_id,database_uri FROM connection") {
        Ok(query) => query,
        Err(error) if error.sqlite_error_code() == Some(rusqlite::ErrorCode::TooBig) => {
            return Err(Error::ResourceLimit)
        }
        // The connection lookup is optional in the original source.
        Err(_) => return Ok(out),
    };
    let mut rows = query.query([]).map_err(|error| {
        if error.sqlite_error_code() == Some(rusqlite::ErrorCode::TooBig) {
            Error::ResourceLimit
        } else {
            Error::InvalidData
        }
    })?;
    let (mut count, mut bytes) = (0usize, 0usize);
    loop {
        if cancelled.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            return Err(Error::Stopped);
        }
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(error) if error.sqlite_error_code() == Some(rusqlite::ErrorCode::TooBig) => {
                return Err(Error::ResourceLimit)
            }
            Err(_) => return Ok(out),
        };
        count = count.checked_add(1).ok_or(Error::ResourceLimit)?;
        if count > MAX_MAPPING_ROWS {
            return Err(Error::ResourceLimit);
        }
        let value = match row.get_ref(1).map_err(|_| Error::InvalidData)? {
            rusqlite::types::ValueRef::Text(value) => value,
            _ => continue,
        };
        bytes = bytes.checked_add(value.len()).ok_or(Error::ResourceLimit)?;
        if bytes > MAX_MAPPING_URI_BYTES {
            return Err(Error::ResourceLimit);
        }
        let Ok(id) = row.get::<_, i64>(0) else {
            continue;
        };
        let uri = String::from_utf8_lossy(value);
        if uri.ends_with("/root.sqlite") || uri.ends_with("/beatport.sqlite") {
            out.insert(id, "/".into());
        } else if let Some(root) = uri.strip_suffix("/_Serato_/Library/location.sqlite") {
            if !root.is_empty() {
                out.insert(id, format!("{root}/"));
            }
        }
    }
    Ok(out)
}
fn songs(
    db: &Connection,
    session: i64,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<SeratoHistorySong>> {
    let maps = mappings(db, cancelled)?;
    let mut query=db.prepare("SELECT name,artist,deck,start_time,end_time,played,bpm,file_name,portable_id FROM history_entry WHERE session_id=? ORDER BY start_time ASC").map_err(|_|Error::InvalidData)?;
    let mut rows = query.query([session]).map_err(|_| Error::InvalidData)?;
    let mut out = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = rows.next().map_err(|_| Error::InvalidData)? {
        if cancelled.is_some_and(|value| value.load(std::sync::atomic::Ordering::Acquire)) {
            return Err(Error::Stopped);
        }
        if out.len() >= 1_000_000 {
            return Err(Error::ResourceLimit);
        }
        let text = |i| -> Result<Option<String>> { row.get(i).map_err(|_| Error::InvalidData) };
        let portable = text(8)?.unwrap_or_default();
        let name = text(0)?.unwrap_or_default();
        let artist = text(1)?.unwrap_or_default();
        let start = row
            .get::<_, Option<f64>>(3)
            .map_err(|_| Error::InvalidData)?
            .unwrap_or(0.0);
        let end = row
            .get::<_, Option<f64>>(4)
            .map_err(|_| Error::InvalidData)?
            .unwrap_or(0.0);
        let stamp = |v: f64| {
            if v > 0.0 && v.is_finite() && v * 1000.0 <= 8_640_000_000_000_000.0 {
                Some((v * 1000.0) as i64)
            } else {
                None
            }
        };
        let mut song = SeratoHistorySong {
            title: name,
            artist,
            file_path: text(7)?
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| portable.clone()),
            bpm: row.get(6).map_err(|_| Error::InvalidData)?,
            start_time: stamp(start),
            play_time: stamp(end),
            played: Some(
                row.get::<_, Option<i64>>(5)
                    .map_err(|_| Error::InvalidData)?
                    == Some(1),
            ),
            deck: text(2)?.as_deref().and_then(parse_deck_value),
            ..Default::default()
        };
        if !portable.is_empty() {
            if let Ok(mut asset)=db.prepare("SELECT location_id,portable_id,type_specific_data,album,label,genre,key FROM asset WHERE portable_id=? LIMIT 1"){
            if let Ok(mut assets)=asset.query([&portable]){if let Ok(Some(row))=assets.next(){
                let text=|i|row.get::<_,Option<String>>(i).ok().flatten().filter(|s|!s.is_empty());
                if let Some((source,id))=parse_streaming_id(&portable){song.file_path=portable.clone();song.streaming_source=Some(source.into());song.streaming_id=Some(id.into());
                    song.artwork_url=text(2).and_then(|s|serde_json::from_str::<serde_json::Value>(&s).ok()).and_then(|v|v.get("cover_id").and_then(|v|v.as_str()).filter(|s|!s.is_empty()).map(str::to_owned));
                }else if let Ok(location)=row.get::<_,i64>(0){if let Some(base)=maps.get(&location){song.file_path=format!("{base}{portable}");}}
                song.album=text(3);song.label=text(4);song.genre=text(5);song.key=text(6);
            }}
        }
        }
        bytes = bytes.saturating_add(
            serde_json::to_vec(&song)
                .map_err(|_| Error::InvalidData)?
                .len(),
        );
        if bytes > 128 << 20 {
            return Err(Error::ResourceLimit);
        }
        out.push(song);
    }
    Ok(out)
}
pub fn parse_deck_value(text: &str) -> Option<u32> {
    let numeric = text.trim_start_matches(crate::util::js_whitespace);
    let prefix = usize::from(numeric.starts_with(['+', '-']));
    let count = numeric[prefix..]
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    if count > 0 {
        if let Ok(value) = numeric[..prefix + count].parse::<i64>() {
            if (1..=4).contains(&value) {
                return Some(value as u32);
            }
        }
    }
    match text.to_ascii_uppercase().as_str() {
        "A" => Some(1),
        "B" => Some(2),
        "C" => Some(3),
        "D" => Some(4),
        _ => None,
    }
}
fn parse_streaming_id(text: &str) -> Option<(&str, &str)> {
    let (source, id) = text.strip_prefix("streaming://")?.split_once('/')?;
    if source.is_empty()
        || id.is_empty()
        || !source
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        None
    } else {
        Some((source, id))
    }
}
pub fn get_latest_session_songs_v4(path: Option<&Path>) -> Result<Vec<SeratoHistorySong>> {
    let Some(db) = connection(path)? else {
        return Ok(Vec::new());
    };
    let mut query = db
        .prepare("SELECT id FROM history_session ORDER BY start_time DESC LIMIT 1")
        .map_err(|_| Error::InvalidData)?;
    let mut rows = query.query([]).map_err(|_| Error::InvalidData)?;
    let Some(row) = rows.next().map_err(|_| Error::InvalidData)? else {
        return Ok(Vec::new());
    };
    songs(&db, row.get(0).map_err(|_| Error::InvalidData)?, None)
}
pub fn get_serato_v4_sessions(path: Option<&Path>) -> Result<Vec<SeratoSession>> {
    let Some(db) = connection(path)? else {
        return Ok(Vec::new());
    };
    let mut query = db
        .prepare("SELECT id,start_time FROM history_session ORDER BY start_time DESC")
        .map_err(|_| Error::InvalidData)?;
    let mut rows = query.query([]).map_err(|_| Error::InvalidData)?;
    let mut out = Vec::new();
    let (mut bytes, mut entries) = (0usize, 0usize);
    while let Some(row) = rows.next().map_err(|_| Error::InvalidData)? {
        if out.len() >= 1_000_000 {
            return Err(Error::ResourceLimit);
        }
        let id = row.get(0).map_err(|_| Error::InvalidData)?;
        let stamp = row.get::<_, f64>(1).map_err(|_| Error::InvalidData)?;
        let date = chrono::DateTime::from_timestamp_millis((stamp * 1000.0) as i64)
            .ok_or(Error::InvalidData)?
            .format("%Y-%m-%d")
            .to_string();
        let session = SeratoSession {
            date,
            index: id,
            songs: songs(&db, id, None)?,
        };
        bytes = bytes.saturating_add(
            serde_json::to_vec(&session)
                .map_err(|_| Error::InvalidData)?
                .len(),
        );
        entries = entries.saturating_add(session.songs.len());
        if bytes > 128 << 20 || entries > 1_000_000 {
            return Err(Error::ResourceLimit);
        }
        out.push(session);
    }
    Ok(out)
}
pub fn get_database_mtime(path: Option<&Path>) -> Result<f64> {
    let path = get_master_database_path(path)?;
    if !path.exists() {
        return Ok(0.0);
    }
    Ok(std::fs::metadata(path)?
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0))
}

/// One short-lived read snapshot for the actor: session identity and songs
/// cannot straddle a concurrent session switch between separate connections.
pub(crate) fn latest_snapshot_v4(
    path: &Path,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Option<(LatestSessionInfo, Vec<SeratoHistorySong>)>> {
    use rusqlite::OptionalExtension;
    let Some(db) = connection(Some(path))? else {
        return Ok(None);
    };
    let tx = db.unchecked_transaction().map_err(|_| Error::InvalidData)?;
    let id = tx
        .query_row(
            "SELECT id FROM history_session ORDER BY start_time DESC LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|_| Error::InvalidData)?;
    let Some(id) = id else { return Ok(None) };
    let rows = songs(&tx, id, cancelled)?;
    tx.commit().map_err(|_| Error::InvalidData)?;
    Ok(Some((
        LatestSessionInfo {
            id,
            entry_count: rows.len(),
        },
        rows,
    )))
}
