//! Audio container readers for Serato-only metadata. Audio payload is skipped
//! using Seek; only bounded tag/header blocks enter memory. No audio decoder,
//! network request, cover image extraction or media modification is performed.
use crate::{
    encoding::decode_with_linebreaks,
    parsers::*,
    util::{check_size, open_regular, MAX_DATA, MAX_ITEMS},
    Error, Result, SeratoTrackMetadata,
};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Seek, SeekFrom},
    path::Path,
};
const MAX_TAG: usize = 16 * 1024 * 1024;
#[derive(Clone)]
struct Tag {
    name: String,
    data: Vec<u8>,
}
struct Input<R> {
    reader: R,
    len: u64,
    read: usize,
}
impl<R: Read + Seek> Input<R> {
    fn new(mut reader: R) -> Result<Self> {
        let len = reader.seek(SeekFrom::End(0))?;
        Ok(Self {
            reader,
            len,
            read: 0,
        })
    }
    fn bytes(&mut self, offset: u64, len: usize) -> Result<Vec<u8>> {
        if len > MAX_TAG || self.read.checked_add(len).is_none_or(|v| v > MAX_DATA) {
            return Err(Error::ResourceLimit);
        }
        if offset.checked_add(len as u64).is_none_or(|v| v > self.len) {
            return Err(Error::InvalidData);
        }
        self.reader.seek(SeekFrom::Start(offset))?;
        let mut data = vec![0; len];
        self.reader.read_exact(&mut data)?;
        self.read += len;
        Ok(data)
    }
}
fn be32(data: &[u8]) -> Result<u32> {
    Ok(u32::from_be_bytes(
        data.get(..4)
            .ok_or(Error::InvalidData)?
            .try_into()
            .map_err(|_| Error::InvalidData)?,
    ))
}
fn le32(data: &[u8]) -> Result<u32> {
    Ok(u32::from_le_bytes(
        data.get(..4)
            .ok_or(Error::InvalidData)?
            .try_into()
            .map_err(|_| Error::InvalidData)?,
    ))
}
fn synchsafe(data: &[u8]) -> Result<u32> {
    let bytes = data.get(..4).ok_or(Error::InvalidData)?;
    if bytes.iter().any(|v| v & 0x80 != 0) {
        return Err(Error::InvalidData);
    }
    Ok(bytes.iter().fold(0, |n, v| (n << 7) | u32::from(*v)))
}
fn deunsync(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        out.push(data[i]);
        if data[i] == 0xff && data.get(i + 1) == Some(&0) {
            i += 1;
        }
        i += 1;
    }
    out
}
fn encoded_string(data: &[u8], encoding: u8) -> Result<(String, usize)> {
    if encoding == 1 || encoding == 2 {
        let end = data
            .as_chunks::<2>()
            .0
            .iter()
            .position(|v| *v == [0, 0])
            .map(|i| i * 2)
            .ok_or(Error::InvalidData)?;
        let mut body = &data[..end];
        let mut little = false;
        if encoding == 1 {
            if body.starts_with(&[0xff, 0xfe]) {
                little = true;
                body = &body[2..];
            } else if body.starts_with(&[0xfe, 0xff]) {
                body = &body[2..];
            }
        }
        let codes = body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| {
                if little {
                    u16::from_le_bytes([v[0], v[1]])
                } else {
                    u16::from_be_bytes([v[0], v[1]])
                }
            })
            .collect::<Vec<_>>();
        Ok((String::from_utf16_lossy(&codes), end + 2))
    } else {
        let end = data
            .iter()
            .position(|v| *v == 0)
            .ok_or(Error::InvalidData)?;
        let s = if encoding == 0 {
            data[..end].iter().map(|v| char::from(*v)).collect()
        } else if encoding == 3 {
            String::from_utf8_lossy(&data[..end]).into_owned()
        } else {
            return Err(Error::InvalidData);
        };
        Ok((s, end + 1))
    }
}
fn geob(body: &[u8]) -> Result<Tag> {
    let encoding = *body.first().ok_or(Error::InvalidData)?;
    let body = &body[1..];
    let mime = body
        .iter()
        .position(|v| *v == 0)
        .ok_or(Error::InvalidData)?;
    let body = &body[mime + 1..];
    let (_, filename) = encoded_string(body, encoding)?;
    let body = &body[filename..];
    let (name, description) = encoded_string(body, encoding)?;
    Ok(Tag {
        name,
        data: body[description..].to_vec(),
    })
}
fn id3<R: Read + Seek>(input: &mut Input<R>, offset: u64) -> Result<Vec<Tag>> {
    let header = input.bytes(offset, 10)?;
    if &header[..3] != b"ID3" {
        return Ok(Vec::new());
    }
    let version = header[3];
    if !(2..=4).contains(&version) {
        return Err(Error::UnsupportedFormat);
    }
    let mut data = input.bytes(offset + 10, synchsafe(&header[6..10])? as usize)?;
    if header[5] & 0x80 != 0 {
        data = deunsync(&data);
    }
    let mut start = 0;
    if header[5] & 0x40 != 0 && version >= 3 {
        let size = if version == 4 {
            synchsafe(&data)? as usize
        } else {
            be32(&data)? as usize + 4
        };
        if size > data.len() {
            return Err(Error::InvalidData);
        }
        start = size;
    }
    let mut tags = Vec::new();
    let mut cursor = start;
    let mut count = 0;
    while cursor < data.len() {
        let head = if version == 2 { 6 } else { 10 };
        if data.len() - cursor < head {
            break;
        }
        let id_len = if version == 2 { 3 } else { 4 };
        let id = &data[cursor..cursor + id_len];
        if id.iter().all(|v| *v == 0) {
            break;
        }
        let size = if version == 2 {
            ((data[cursor + 3] as usize) << 16)
                | ((data[cursor + 4] as usize) << 8)
                | data[cursor + 5] as usize
        } else if version == 4 {
            synchsafe(&data[cursor + 4..cursor + 8])? as usize
        } else {
            be32(&data[cursor + 4..cursor + 8])? as usize
        };
        let begin = cursor + head;
        let end = begin.checked_add(size).ok_or(Error::ResourceLimit)?;
        let Some(body) = data.get(begin..end) else {
            break;
        };
        count += 1;
        if count > MAX_ITEMS {
            return Err(Error::ResourceLimit);
        }
        if id == b"GEOB" || id == b"GEO" {
            let flags = if version == 2 { 0 } else { data[cursor + 9] };
            // Compressed/encrypted frames need a separate decoder; never parse their
            // compressed payload as Serato metadata. Other frames remain usable.
            let blocked = if version == 3 {
                flags & 0xc0 != 0
            } else {
                flags & 0x0c != 0
            };
            if !blocked {
                let mut body = body.to_vec();
                if version == 4 && flags & 2 != 0 {
                    body = deunsync(&body);
                }
                let prefix = if version == 3 {
                    usize::from(flags & 0x20 != 0)
                } else {
                    usize::from(flags & 0x40 != 0) + if flags & 1 != 0 { 4 } else { 0 }
                };
                if let Some(body) = body.get(prefix..) {
                    if let Ok(tag) = geob(body) {
                        tags.push(tag);
                    }
                }
            }
        }
        cursor = end;
    }
    Ok(tags)
}
fn vorbis(data: &[u8]) -> Result<Vec<Tag>> {
    let mut offset = 0usize;
    let vendor = le32(data)? as usize;
    offset = offset.checked_add(4 + vendor).ok_or(Error::ResourceLimit)?;
    let count = le32(data.get(offset..).ok_or(Error::InvalidData)?)? as usize;
    offset += 4;
    if count > MAX_ITEMS {
        return Err(Error::ResourceLimit);
    }
    let mut tags = Vec::new();
    for _ in 0..count {
        let size = le32(data.get(offset..).ok_or(Error::InvalidData)?)? as usize;
        offset += 4;
        let end = offset.checked_add(size).ok_or(Error::ResourceLimit)?;
        let text = String::from_utf8_lossy(data.get(offset..end).ok_or(Error::InvalidData)?);
        if let Some((name, value)) = text.split_once('=') {
            tags.push(Tag {
                name: name.to_uppercase(),
                data: value.as_bytes().to_vec(),
            });
        }
        offset = end;
    }
    Ok(tags)
}
fn flac<R: Read + Seek>(input: &mut Input<R>) -> Result<Vec<Tag>> {
    let mut offset = 4;
    let mut count = 0;
    loop {
        count += 1;
        if count > MAX_ITEMS {
            return Err(Error::ResourceLimit);
        }
        let head = input.bytes(offset, 4)?;
        let size = ((head[1] as usize) << 16) | ((head[2] as usize) << 8) | head[3] as usize;
        if head[0] & 0x7f == 4 {
            return vorbis(&input.bytes(offset + 4, size)?);
        }
        offset += 4 + size as u64;
        if head[0] & 0x80 != 0 {
            return Ok(Vec::new());
        }
    }
}
fn aiff<R: Read + Seek>(input: &mut Input<R>) -> Result<Vec<Tag>> {
    let mut offset = 12;
    let mut count = 0;
    while offset + 8 <= input.len {
        count += 1;
        if count > MAX_ITEMS {
            return Err(Error::ResourceLimit);
        }
        let head = input.bytes(offset, 8)?;
        let size = u64::from(be32(&head[4..])?);
        if &head[..4] == b"ID3 " {
            return id3(input, offset + 8);
        }
        offset = offset
            .checked_add(8 + size + (size & 1))
            .ok_or(Error::ResourceLimit)?;
    }
    Ok(Vec::new())
}
fn wav<R: Read + Seek>(input: &mut Input<R>) -> Result<Vec<Tag>> {
    let mut offset = 12;
    let mut count = 0;
    while offset + 8 <= input.len {
        count += 1;
        if count > MAX_ITEMS {
            return Err(Error::ResourceLimit);
        }
        let head = input.bytes(offset, 8)?;
        let size = u64::from(le32(&head[4..])?);
        if matches!(&head[..4], b"id3 " | b"ID3 ") {
            return id3(input, offset + 8);
        }
        offset = offset
            .checked_add(8 + size + (size & 1))
            .ok_or(Error::ResourceLimit)?;
    }
    Ok(Vec::new())
}
fn ogg<R: Read + Seek>(input: &mut Input<R>) -> Result<Vec<Tag>> {
    let mut offset = 0;
    let mut packets: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    let mut count = 0;
    while offset + 27 <= input.len {
        count += 1;
        if count > 10000 {
            return Err(Error::ResourceLimit);
        }
        let head = input.bytes(offset, 27)?;
        if &head[..4] != b"OggS" || head[4] != 0 {
            return Err(Error::InvalidData);
        }
        let serial = le32(&head[14..18])?;
        let laces = input.bytes(offset + 27, head[26] as usize)?;
        let size: usize = laces.iter().map(|v| *v as usize).sum();
        let body = input.bytes(offset + 27 + laces.len() as u64, size)?;
        let packet = packets.entry(serial).or_default();
        let mut cursor = 0;
        for lace in laces.iter().copied() {
            let n = lace as usize;
            if packet.len() + n > MAX_TAG {
                return Err(Error::ResourceLimit);
            }
            packet.extend_from_slice(&body[cursor..cursor + n]);
            cursor += n;
            if lace < 255 {
                if packet.starts_with(b"\x03vorbis") {
                    return vorbis(&packet[7..]);
                }
                if packet.starts_with(b"OpusTags") {
                    return vorbis(&packet[8..]);
                }
                packet.clear();
            }
        }
        if packets.len() > 64 {
            return Err(Error::ResourceLimit);
        }
        offset += 27 + laces.len() as u64 + size as u64;
    }
    Ok(Vec::new())
}
fn mp4<R: Read + Seek>(
    input: &mut Input<R>,
    start: u64,
    end: u64,
    depth: usize,
    tags: &mut Vec<Tag>,
) -> Result<()> {
    if depth > 16 {
        return Err(Error::ResourceLimit);
    }
    let mut offset = start;
    let mut count = 0;
    while offset + 8 <= end {
        count += 1;
        if count > MAX_ITEMS {
            return Err(Error::ResourceLimit);
        }
        let head = input.bytes(offset, 8)?;
        let mut header = 8;
        let mut size = u64::from(be32(&head)?);
        if size == 1 {
            let big = input.bytes(offset + 8, 8)?;
            size = u64::from_be_bytes(big.try_into().map_err(|_| Error::InvalidData)?);
            header = 16;
        } else if size == 0 {
            size = end - offset;
        }
        if size < header || size > end - offset {
            return Err(Error::InvalidData);
        }
        let body = offset + header;
        let atom_end = offset + size;
        match &head[4..8] {
            b"moov" | b"udta" | b"ilst" => mp4(input, body, atom_end, depth + 1, tags)?,
            b"meta" => {
                if atom_end - body < 4 {
                    return Err(Error::InvalidData);
                }
                // QuickTime permits a meta container without the FullBox
                // prefix when the first child is hdlr. Preserve that variant.
                let child = if atom_end - body >= 8 {
                    let head = input.bytes(body, 8)?;
                    if &head[4..8] == b"hdlr" {
                        body
                    } else {
                        body + 4
                    }
                } else {
                    body + 4
                };
                mp4(input, child, atom_end, depth + 1, tags)?
            }
            b"----" => {
                let mut p = body;
                let mut mean = None;
                let mut name = None;
                let mut value = None;
                while p + 8 <= atom_end {
                    let h = input.bytes(p, 8)?;
                    let n = u64::from(be32(&h)?);
                    if n < 8 || n > atom_end - p {
                        return Err(Error::InvalidData);
                    }
                    let d = input
                        .bytes(p + 8, (n - 8).try_into().map_err(|_| Error::ResourceLimit)?)?;
                    match &h[4..8] {
                        b"mean" if d.len() >= 4 => {
                            mean = Some(String::from_utf8_lossy(&d[4..]).into_owned())
                        }
                        b"name" if d.len() >= 4 => {
                            name = Some(String::from_utf8_lossy(&d[4..]).into_owned())
                        }
                        b"data" if d.len() >= 8 && be32(&d[..4])? & 0xffffff == 0 => {
                            value = Some(d[8..].to_vec())
                        }
                        _ => {}
                    }
                    p += n;
                }
                if mean.as_deref() == Some("com.serato.dj") {
                    if let (Some(name), Some(data)) = (name, value) {
                        tags.push(Tag { name, data });
                    }
                }
            }
            _ => {}
        }
        offset = atom_end;
    }
    Ok(())
}
fn first<'a>(tags: &'a [Tag], name: &str) -> Option<&'a [u8]> {
    tags.iter()
        .find(|t| t.name == name)
        .map(|t| t.data.as_slice())
}
fn metadata(tags: &[Tag], kind: u8) -> SeratoTrackMetadata {
    let mut result = SeratoTrackMetadata::default();
    let names = match kind {
        0 => [
            "Serato Markers2",
            "Serato BeatGrid",
            "Serato Autotags",
            "Serato Overview",
        ],
        1 => [
            "SERATO_MARKERS_V2",
            "SERATO_BEATGRID",
            "SERATO_AUTOTAGS",
            "SERATO_OVERVIEW",
        ],
        _ => ["markersv2", "beatgrid", "autotags", "overview"],
    };
    // Vorbis values are strings in the original API: an empty comment is
    // falsey and must not manufacture empty Serato tag objects.
    let tag = |name| first(tags, name).filter(|data| kind != 1 || !data.is_empty());
    if let Some(data) = tag(names[0]) {
        result.markers = match kind {
            0 => parse_markers2_from_geob(data).ok(),
            1 => parse_markers2_from_base64(&String::from_utf8_lossy(data)).ok(),
            _ => parse_markers2(data).ok(),
        };
    }
    if let Some(data) = tag(names[1]) {
        result.beatgrid = if kind == 1 {
            parse_beatgrid_from_base64(&String::from_utf8_lossy(data)).ok()
        } else {
            parse_beatgrid(data).ok()
        };
    }
    if let Some(data) = tag(names[2]) {
        result.autotags = if kind == 1 {
            parse_autotags_from_base64(&String::from_utf8_lossy(data)).ok()
        } else {
            parse_autotags(data).ok()
        };
    }
    if let Some(data) = tag(names[3]) {
        result.overview = if kind == 1 {
            decode_with_linebreaks(&String::from_utf8_lossy(data)).ok()
        } else {
            Some(data.to_vec())
        };
    }
    result
}
fn read_metadata<R: Read + Seek>(reader: R) -> Result<Option<SeratoTrackMetadata>> {
    let mut input = Input::new(reader)?;
    if input.len < 4 {
        return Ok(None);
    }
    let head = input.bytes(0, input.len.min(12) as usize)?;
    let result = if head.starts_with(b"ID3") {
        metadata(&id3(&mut input, 0)?, 0)
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WAVE" {
        metadata(&wav(&mut input)?, 0)
    } else if head.starts_with(b"fLaC") {
        metadata(&flac(&mut input)?, 1)
    } else if head.starts_with(b"OggS") {
        metadata(&ogg(&mut input)?, 1)
    } else if head.len() >= 12 && &head[..4] == b"FORM" && matches!(&head[8..12], b"AIFF" | b"AIFC")
    {
        metadata(&aiff(&mut input)?, 0)
    } else if head.len() >= 8
        && matches!(&head[4..8], b"ftyp" | b"moov" | b"free" | b"wide" | b"mdat")
    {
        let mut tags = Vec::new();
        let len = input.len;
        mp4(&mut input, 0, len, 0, &mut tags)?;
        metadata(&tags, 2)
    } else {
        return Ok(None);
    };
    if result.markers.is_none()
        && result.beatgrid.is_none()
        && result.autotags.is_none()
        && result.overview.is_none()
    {
        Ok(None)
    } else {
        Ok(Some(result))
    }
}
/// Source-compatible null on unreadable/invalid/unsupported media. A resource
/// bound remains an explicit error, so callers cannot mistake it for no tags.
pub fn get_track_metadata(path: impl AsRef<Path>) -> Result<Option<SeratoTrackMetadata>> {
    match open_regular(path.as_ref()).and_then(read_metadata) {
        Err(Error::ResourceLimit) => Err(Error::ResourceLimit),
        Err(_) => Ok(None),
        value => value,
    }
}
pub fn get_track_metadata_from_buffer(
    data: &[u8],
    mime_type: &str,
) -> Result<Option<SeratoTrackMetadata>> {
    check_size(data)?;
    // The original buffer reader honors recognized MIME hints. An unknown
    // hint is sniffed, but a recognized contradictory hint must not silently
    // select another format and return metadata the TS reader rejects.
    let mime = mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let compatible = match mime.as_str() {
        "audio/mpeg" | "audio/mp3" => data.starts_with(b"ID3") || data.first() == Some(&0xff),
        "audio/flac" | "audio/x-flac" => data.starts_with(b"fLaC"),
        "audio/ogg" | "video/ogg" | "application/ogg" | "audio/vorbis" => data.starts_with(b"OggS"),
        "audio/aiff" | "audio/x-aiff" => data.starts_with(b"FORM"),
        "audio/wav" | "audio/x-wav" | "audio/vnd.wave" => data.starts_with(b"RIFF"),
        "audio/mp4" | "video/mp4" | "audio/x-m4a" => {
            data.len() >= 8
                && matches!(&data[4..8], b"ftyp" | b"moov" | b"free" | b"wide" | b"mdat")
        }
        _ => true,
    };
    if !compatible {
        return Ok(None);
    }

    match read_metadata(Cursor::new(data)) {
        Err(Error::ResourceLimit) => Err(Error::ResourceLimit),
        Err(_) => Ok(None),
        value => value,
    }
}
