//! Serato 3 binary History parser. Inputs are explicit; no profile discovery.
use crate::{
    util::{be_u32, read_file, utf16be, MAX_DATA, MAX_ITEMS},
    Error, Result, SeratoHistorySong, SeratoSession,
};
use std::{
    fs,
    path::{Path, PathBuf},
};
pub const CHUNK_TAGS: [(&str, [u8; 4]); 14] = [
    ("OSES", *b"oses"),
    ("OENT", *b"oent"),
    ("OTRK", *b"otrk"),
    ("ADAT", *b"adat"),
    ("INDEX", [0, 0, 0, 1]),
    ("BPM", [0, 0, 0, 15]),
    ("START_TIME", [0, 0, 0, 28]),
    ("DECK", [0, 0, 0, 31]),
    ("PLAYED", [0, 0, 0, 50]),
    ("PLAY_TIME", [0, 0, 0, 45]),
    ("TITLE", [0, 0, 0, 6]),
    ("ARTIST", [0, 0, 0, 7]),
    ("FILE_PATH", [0, 0, 0, 2]),
    ("SESSION_DATE", [0, 0, 0, 41]),
];
fn chunks(data: &[u8]) -> Result<Vec<([u8; 4], &[u8])>> {
    if data.len() > MAX_DATA {
        return Err(Error::ResourceLimit);
    }
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        if out.len() >= MAX_ITEMS {
            return Err(Error::ResourceLimit);
        }
        let tag = data
            .get(at..at.saturating_add(4))
            .ok_or(Error::InvalidData)?
            .try_into()
            .map_err(|_| Error::InvalidData)?;
        let length = be_u32(data, at + 4)? as usize;
        at += 8;
        let end = at.checked_add(length).ok_or(Error::ResourceLimit)?;
        out.push((tag, data.get(at..end).ok_or(Error::InvalidData)?));
        at = end;
    }
    Ok(out)
}
pub fn parse_session_bytes(data: &[u8]) -> Result<Vec<SeratoHistorySong>> {
    let mut songs = Vec::new();
    for (tag, entry) in chunks(data)? {
        if tag != *b"oent" {
            continue;
        }
        let nested = chunks(entry)?;
        let Some((tag, data)) = nested.first() else {
            continue;
        };
        if tag != b"adat" {
            continue;
        }
        let mut song = SeratoHistorySong::default();
        let (mut played, mut deck) = (false, 0);
        for (tag, value) in chunks(data)? {
            match tag {
                [0, 0, 0, 6] => song.title = utf16be(value),
                [0, 0, 0, 7] => song.artist = utf16be(value),
                [0, 0, 0, 2] => song.file_path = utf16be(value),
                [0, 0, 0, 15] => song.bpm = Some(be_u32(value, 0)? as f64),
                [0, 0, 0, 28] => song.start_time = Some(i64::from(be_u32(value, 0)?) * 1000),
                [0, 0, 0, 45] => song.play_time = Some(i64::from(be_u32(value, 0)?) * 1000),
                [0, 0, 0, 50] => played = *value.first().ok_or(Error::InvalidData)? != 0,
                [0, 0, 0, 31] => deck = be_u32(value, 0)?,
                _ => {}
            }
        }
        song.played = Some(played);
        song.playing = Some(played && song.start_time.is_some() && song.play_time.is_none());
        song.deck = Some(deck);
        songs.push(song);
    }
    Ok(songs)
}
/// Ordered date/index pairs preserve JS Map insertion order and overwrite rules.
pub fn parse_sessions_bytes(data: &[u8]) -> Result<Vec<(String, i64)>> {
    let mut out: Vec<(String, i64)> = Vec::new();
    for (tag, entry) in chunks(data)? {
        if tag != *b"oses" {
            continue;
        }
        let entries = chunks(entry)?;
        let Some((tag, inner)) = entries.first() else {
            continue;
        };
        if tag != b"adat" {
            continue;
        }
        let (mut date, mut index) = (String::new(), None);
        for (tag, value) in chunks(inner)? {
            match tag {
                [0, 0, 0, 1] => index = Some(i64::from(be_u32(value, 0)?)),
                [0, 0, 0, 41] => date = utf16be(value),
                _ => {}
            }
        }
        if !date.is_empty() {
            if let Some(index) = index {
                if let Some((_, old)) = out.iter_mut().find(|(key, _)| key == &date) {
                    *old = index
                } else {
                    out.push((date, index))
                }
            }
        }
    }
    Ok(out)
}
pub fn get_sessions(path: impl AsRef<Path>) -> Result<Vec<(String, i64)>> {
    parse_sessions_bytes(&read_file(path.as_ref())?)
}
pub fn get_session_songs(path: impl AsRef<Path>) -> Result<Vec<SeratoHistorySong>> {
    parse_session_bytes(&read_file(path.as_ref())?)
}
pub fn get_serato_history(path: impl AsRef<Path>) -> Result<Vec<SeratoSession>> {
    let root = path.as_ref();
    let database = root.join("History/history.database");
    if !database.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    let (mut bytes, mut rows) = (0usize, 0usize);
    for (date, index) in get_sessions(database)? {
        let path = root.join(format!("History/Sessions/{index}.session"));
        if path.exists() {
            let session = SeratoSession {
                date,
                index,
                songs: get_session_songs(path)?,
            };
            bytes = bytes.saturating_add(
                serde_json::to_vec(&session)
                    .map_err(|_| Error::InvalidData)?
                    .len(),
            );
            rows = rows.saturating_add(session.songs.len());
            if bytes > 128 << 20 || rows > MAX_ITEMS {
                return Err(Error::ResourceLimit);
            }
            sessions.push(session)
        }
    }
    Ok(sessions)
}
pub fn get_latest_session_path(path: impl AsRef<Path>) -> Result<Option<PathBuf>> {
    let dir = path.as_ref().join("History/Sessions");
    if !dir.exists() {
        return Ok(None);
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(name) = name.strip_suffix(".session") else {
            continue;
        };
        let text = name.trim_start();
        let mut end = usize::from(text.starts_with(['+', '-']));
        let start = end;
        while text.as_bytes().get(end).is_some_and(u8::is_ascii_digit) {
            end += 1
        }
        if end > start {
            if let Ok(index) = text[..end].parse::<i64>() {
                files.push((index, entry.path()))
            }
        }
        if files.len() > MAX_ITEMS {
            return Err(Error::ResourceLimit);
        }
    }
    files.sort_by_key(|value| std::cmp::Reverse(value.0));
    Ok(files.into_iter().next().map(|(_, p)| p))
}
