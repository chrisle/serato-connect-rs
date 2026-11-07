use crate::{Error, Result};
use std::{
    fs::File,
    io::Read,
    path::{Component, Path},
};
pub const MAX_DATA: usize = 64 * 1024 * 1024;
pub const MAX_ITEMS: usize = 1_000_000;
pub fn check_size(data: &[u8]) -> Result<()> {
    if data.len() > MAX_DATA {
        Err(Error::ResourceLimit)
    } else {
        Ok(())
    }
}
pub fn open_regular(path: &Path) -> Result<File> {
    #[cfg(windows)]
    if windows_device_path(&path.to_string_lossy()) {
        return Err(Error::InvalidData);
    }
    if !std::fs::metadata(path)?.is_file() {
        return Err(Error::InvalidData);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::InvalidData);
    }
    Ok(file)
}
pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    let file = open_regular(path)?;
    if file.metadata()?.len() > MAX_DATA as u64 {
        return Err(Error::ResourceLimit);
    }
    let mut data = Vec::new();
    file.take(MAX_DATA as u64 + 1).read_to_end(&mut data)?;
    check_size(&data)?;
    Ok(data)
}

pub fn be_u32(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        data.get(offset..offset.checked_add(4).ok_or(Error::ResourceLimit)?)
            .ok_or(Error::InvalidData)?
            .try_into()
            .map_err(|_| Error::InvalidData)?,
    ))
}
pub fn be_f32(data: &[u8], offset: usize) -> Result<f64> {
    Ok(f32::from_bits(be_u32(data, offset)?) as f64)
}
pub fn be_f64(data: &[u8], offset: usize) -> Result<f64> {
    Ok(f64::from_be_bytes(
        data.get(offset..offset.checked_add(8).ok_or(Error::ResourceLimit)?)
            .ok_or(Error::InvalidData)?
            .try_into()
            .map_err(|_| Error::InvalidData)?,
    ))
}
pub fn utf16be(data: &[u8]) -> String {
    String::from_utf16_lossy(
        &data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u16::from_be_bytes([p[0], p[1]]))
            .filter(|v| *v != 0)
            .collect::<Vec<_>>(),
    )
}
pub fn c_string(data: &[u8]) -> String {
    String::from_utf8_lossy(data.split(|v| *v == 0).next().unwrap_or_default()).into_owned()
}
pub fn js_whitespace(c: char) -> bool {
    matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
pub fn parse_float(text: &str) -> Option<f64> {
    let text = text.trim_start_matches(js_whitespace);
    let bytes = text.as_bytes();
    let mut end = usize::from(bytes.first().is_some_and(|v| matches!(v, b'+' | b'-')));
    let digits_start = end;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    let mut digits = end - digits_start;
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        let start = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        digits += end - start;
    }
    if digits == 0 {
        return None;
    }
    if bytes.get(end).is_some_and(|v| matches!(v, b'e' | b'E')) {
        let before = end;
        end += 1;
        if bytes.get(end).is_some_and(|v| matches!(v, b'+' | b'-')) {
            end += 1;
        }
        let start = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if start == end {
            end = before;
        }
    }
    text[..end].parse().ok()
}
pub fn parse_int(text: &str) -> Option<f64> {
    let text = text.trim_start_matches(js_whitespace);
    let bytes = text.as_bytes();
    let mut end = usize::from(bytes.first().is_some_and(|v| matches!(v, b'+' | b'-')));
    let start = end;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if end == start {
        None
    } else {
        text[..end].parse().ok()
    }
}
pub fn normalized_path(path: &str) -> String {
    let mut result = std::path::PathBuf::new();
    for part in Path::new(path).components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if result.file_name().is_some_and(|v| v != "..") {
                    result.pop();
                } else if !result.has_root() {
                    result.push("..");
                }
            }
            part => result.push(part.as_os_str()),
        }
    }
    if result.as_os_str().is_empty() {
        ".".into()
    } else {
        let mut normalized = result.to_string_lossy().to_lowercase();
        let trailing =
            path.ends_with(std::path::MAIN_SEPARATOR) || cfg!(windows) && path.ends_with('/');
        if trailing && !normalized.ends_with(std::path::MAIN_SEPARATOR) {
            normalized.push(std::path::MAIN_SEPARATOR);
        }
        normalized
    }
}
pub fn chunks(data: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut remaining = data;
    std::iter::from_fn(move || {
        if remaining.len() < 8 {
            return None;
        }
        let length = u32::from_be_bytes(remaining[4..8].try_into().ok()?) as usize;
        if length > remaining.len() - 8 {
            remaining = &[];
            return None;
        }
        let tag = &remaining[..4];
        let body = &remaining[8..8 + length];
        remaining = &remaining[8 + length..];
        Some((tag, body))
    })
}

/// JavaScript Array.sort compares UTF16 code units rather than Unicode scalars.
pub fn js_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

#[cfg(any(windows, test))]
fn windows_device_path(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.starts_with(r"\\.\")
        || path.starts_with(r"\\?\globalroot")
        || path.starts_with(r"\\?\pipe\")
}
#[cfg(test)]
mod tests {
    #[test]
    fn canonical_windows_files_remain_allowed_but_device_names_are_not() {
        for path in [
            r"C:\Music\track.mp3",
            r"\\?\C:\Music\track.mp3",
            r"\\?\UNC\server\share\track.mp3",
        ] {
            assert!(!super::windows_device_path(path));
        }
        for path in [
            r"\\.\pipe\capture",
            r"\\?\pipe\capture",
            r"\\?\GLOBALROOT\Device\Harddisk0",
        ] {
            assert!(super::windows_device_path(path));
        }
    }
}
