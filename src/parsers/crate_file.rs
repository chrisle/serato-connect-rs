use crate::{
    util::{check_size, chunks, js_cmp, normalized_path, read_file, utf16be, MAX_ITEMS},
    Error, Result, SeratoCrate,
};
use std::path::Path;
pub fn parse_crate_data(data: &[u8]) -> Result<(Option<String>, Vec<String>)> {
    check_size(data)?;
    let mut version = None;
    let mut tracks = Vec::new();
    for (tag, body) in chunks(data) {
        match tag {
            b"vrsn" => version = Some(utf16be(body)),
            b"otrk" => {
                for (tag, field) in chunks(body) {
                    if tag == b"ptrk" {
                        let path = utf16be(field);
                        if path.is_empty() {
                            continue;
                        }
                        tracks.push(path);
                        if tracks.len() > MAX_ITEMS {
                            return Err(Error::ResourceLimit);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok((version, tracks))
}
pub fn parse_crate(path: impl AsRef<Path>) -> Result<SeratoCrate> {
    parse_crate_sync(path)
}
pub fn parse_crate_sync(path: impl AsRef<Path>) -> Result<SeratoCrate> {
    let path = path.as_ref();
    let (version, track_paths) = parse_crate_data(&read_file(path)?)?;
    Ok(SeratoCrate {
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .strip_suffix(".crate")
            .unwrap_or(&path.file_name().unwrap_or_default().to_string_lossy())
            .into(),
        path: path.to_string_lossy().into_owned(),
        version,
        track_paths,
    })
}
pub fn list_crates(path: impl AsRef<Path>) -> Result<Vec<String>> {
    let dir = match std::fs::read_dir(path.as_ref().join("Subcrates")) {
        Ok(v) => v,
        Err(_) => return Ok(Vec::new()),
    };
    let mut result = Vec::new();
    for item in dir.flatten() {
        let name = item.file_name();
        if name.to_string_lossy().ends_with(".crate") {
            result.push(item.path().to_string_lossy().into_owned());
            if result.len() > MAX_ITEMS {
                return Err(Error::ResourceLimit);
            }
        }
    }
    result.sort_by(|a, b| js_cmp(a, b));
    Ok(result)
}
pub fn get_all_crates(path: impl AsRef<Path>) -> Result<Vec<SeratoCrate>> {
    Ok(list_crates(path)?
        .into_iter()
        .filter_map(|p| parse_crate_sync(p).ok())
        .collect())
}
pub fn find_crates_for_track(path: impl AsRef<Path>, track_path: &str) -> Result<Vec<String>> {
    let normalized = normalized_path(track_path);
    let suffix = normalized
        .split(std::path::MAIN_SEPARATOR)
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(std::path::MAIN_SEPARATOR_STR);
    Ok(get_all_crates(path)?
        .into_iter()
        .filter(|c| {
            c.track_paths.iter().any(|p| {
                let p = normalized_path(p);
                p == normalized || normalized.ends_with(&p) || p.ends_with(&suffix)
            })
        })
        .map(|c| c.name)
        .collect())
}
