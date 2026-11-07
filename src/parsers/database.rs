use crate::{
    util::{
        be_u32, check_size, chunks, js_cmp, normalized_path, parse_float, parse_int, read_file,
        utf16be, MAX_ITEMS,
    },
    Error, Result, SeratoDatabaseStats, SeratoDatabaseTrack,
};
use std::{collections::BTreeSet, path::Path};
pub fn parse_database_data(data: &[u8]) -> Result<Vec<SeratoDatabaseTrack>> {
    check_size(data)?;
    let mut result = Vec::new();
    for (tag, body) in chunks(data) {
        if tag != b"otrk" {
            continue;
        }
        let mut t = SeratoDatabaseTrack::default();
        for (key, value) in chunks(body) {
            if value.is_empty() {
                continue;
            }
            let text = || utf16be(value);
            let float = || parse_float(&text()).filter(|v| *v != 0.0);
            let int = || parse_int(&text()).filter(|v| *v != 0.0);
            match key {
                b"pfil" => t.file_path = text(),
                b"tsng" => t.title = Some(text()),
                b"tart" => t.artist = Some(text()),
                b"talb" => t.album = Some(text()),
                b"tgen" => t.genre = Some(text()),
                b"tkey" => t.key = Some(text()),
                b"tbpm" => t.bpm = float(),
                b"tlen" => t.length = float(),
                b"tbit" => t.bitrate = int(),
                b"tsmp" => t.sample_rate = int(),
                b"ttyp" => t.file_type = Some(text()),
                b"bbgl" => t.beatgrid_locked = Some(value[0] != 0),
                b"bmis" => t.missing = Some(value[0] != 0),
                b"uadd" => {
                    t.date_added = be_u32(value, 0)
                        .ok()
                        .filter(|v| *v > 0)
                        .map(|v| i64::from(v) * 1000)
                }
                b"tcom" => t.comment = Some(text()),
                b"tgrp" => t.grouping = Some(text()),
                b"tcmp" => t.composer = Some(text()),
                b"tlbl" => t.label = Some(text()),
                b"ttyr" => t.year = int(),
                _ => {}
            }
        }
        if !t.file_path.is_empty() {
            result.push(t);
            if result.len() > MAX_ITEMS {
                return Err(Error::ResourceLimit);
            }
        }
    }
    Ok(result)
}
pub fn parse_database(path: impl AsRef<Path>) -> Result<Vec<SeratoDatabaseTrack>> {
    parse_database_sync(path)
}
pub fn parse_database_sync(path: impl AsRef<Path>) -> Result<Vec<SeratoDatabaseTrack>> {
    parse_database_data(&read_file(path.as_ref())?)
}
pub fn get_library_tracks(path: impl AsRef<Path>) -> Result<Vec<SeratoDatabaseTrack>> {
    match parse_database_sync(path.as_ref().join("database V2")) {
        Ok(v) => Ok(v),
        Err(Error::ResourceLimit) => Err(Error::ResourceLimit),
        Err(_) => Ok(Vec::new()),
    }
}
pub fn get_track_by_path(
    path: impl AsRef<Path>,
    track: &str,
) -> Result<Option<SeratoDatabaseTrack>> {
    let normalized = normalized_path(track);
    Ok(get_library_tracks(path)?.into_iter().find(|t| {
        let p = normalized_path(&t.file_path);
        p == normalized || p.ends_with(&normalized)
    }))
}
pub fn search_library(path: impl AsRef<Path>, query: &str) -> Result<Vec<SeratoDatabaseTrack>> {
    let query = query.to_lowercase();
    Ok(get_library_tracks(path)?
        .into_iter()
        .filter(|t| {
            [
                t.title.as_deref(),
                t.artist.as_deref(),
                t.album.as_deref(),
                Some(&t.file_path),
                t.genre.as_deref(),
                t.comment.as_deref(),
            ]
            .into_iter()
            .flatten()
            .filter(|v| !v.is_empty())
            .any(|v| v.to_lowercase().contains(&query))
        })
        .collect())
}
pub fn get_database_stats(path: impl AsRef<Path>) -> Result<SeratoDatabaseStats> {
    let tracks = get_library_tracks(path)?;
    let mut genres = BTreeSet::new();
    let mut artists = BTreeSet::new();
    let mut stats = SeratoDatabaseStats {
        total_tracks: tracks.len(),
        ..Default::default()
    };
    for t in tracks {
        stats.missing_tracks += usize::from(t.missing == Some(true));
        stats.locked_beatgrids += usize::from(t.beatgrid_locked == Some(true));
        if let Some(g) = t.genre.filter(|v| !v.is_empty()) {
            genres.insert(g);
        }
        if let Some(a) = t.artist.filter(|v| !v.is_empty()) {
            artists.insert(a);
        }
    }
    stats.genres = genres.into_iter().collect();
    stats.genres.sort_by(|a, b| js_cmp(a, b));
    stats.artists = artists.len();
    Ok(stats)
}
