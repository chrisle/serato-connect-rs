// Copyright (c) 2025 Jan Holthuis
// Copyright (c) 2026 Chris Le
// SPDX-License-Identifier: MPL-2.0
// This Source Code Form is subject to the Mozilla Public License, v. 2.0.
// See LICENSES/MPL-2.0.txt or https://mozilla.org/MPL/2.0/.
//! Native parsers for the original public Serato tag formats. Checked lengths
//! bound every binary read; unknown entries remain forward-compatible.
use crate::{
    encoding::decode_with_linebreaks,
    types::*,
    util::{be_f32, be_f64, be_u32, c_string, check_size, parse_float, MAX_ITEMS},
    Error, Result,
};
fn color(data: &[u8], offset: usize, alpha: Option<u8>) -> Result<SeratoColor> {
    let v = data
        .get(offset..offset.checked_add(3).ok_or(Error::ResourceLimit)?)
        .ok_or(Error::InvalidData)?;
    Ok(SeratoColor {
        r: v[0],
        g: v[1],
        b: v[2],
        a: alpha,
    })
}
fn byte(data: &[u8], offset: usize) -> Result<u8> {
    data.get(offset).copied().ok_or(Error::InvalidData)
}
fn name(data: &[u8], offset: usize) -> Option<String> {
    let value = c_string(data.get(offset..)?);
    (!value.is_empty()).then_some(value)
}
fn parse_cue(data: &[u8]) -> Result<SeratoCuePoint> {
    Ok(SeratoCuePoint {
        index: byte(data, 1)?,
        position: be_u32(data, 2)?,
        color: color(data, 7, None)?,
        name: name(data, 12),
    })
}
fn parse_loop(data: &[u8]) -> Result<SeratoLoop> {
    Ok(SeratoLoop {
        index: byte(data, 1)?,
        start_position: be_u32(data, 2)?,
        end_position: be_u32(data, 6)?,
        color: color(data, 15, Some(byte(data, 14)?))?,
        locked: byte(data, 21)? != 0,
        name: name(data, 22),
    })
}
fn parse_flip(data: &[u8]) -> Result<SeratoFlip> {
    let index = byte(data, 1)?;
    let enabled = byte(data, 2)? != 0;
    let tail = data.get(3..).ok_or(Error::InvalidData)?;
    let end = tail
        .iter()
        .position(|v| *v == 0)
        .ok_or(Error::InvalidData)?;
    let name = String::from_utf8_lossy(&tail[..end]).into_owned();
    let mut offset = 3 + end + 1;
    let looping = byte(data, offset)? != 0;
    offset += 1;
    let count = be_u32(data, offset)? as usize;
    offset += 4;
    if count > MAX_ITEMS {
        return Err(Error::ResourceLimit);
    }
    let mut actions = Vec::new();
    for _ in 0..count {
        let kind = byte(data, offset)?;
        let length = be_u32(data, offset + 1)? as usize;
        offset += 5;
        let body = data
            .get(offset..offset.checked_add(length).ok_or(Error::ResourceLimit)?)
            .ok_or(Error::InvalidData)?;
        if kind <= 1 {
            actions.push(SeratoFlipAction {
                action_type: if kind == 0 {
                    SeratoFlipActionType::Jump
                } else {
                    SeratoFlipActionType::Censor
                },
                source_position: be_f64(body, 0)?,
                target_position: be_f64(body, 8)?,
                speed_factor: if kind == 1 {
                    Some(be_f64(body, 16)?)
                } else {
                    None
                },
            });
        }
        offset += length;
    }
    Ok(SeratoFlip {
        index,
        enabled,
        name,
        looping,
        actions,
    })
}
pub fn parse_markers2(data: &[u8]) -> Result<SeratoMarkers2> {
    check_size(data)?;
    let mut result = SeratoMarkers2::default();
    let mut offset = usize::from(data.starts_with(&[1, 1])) * 2;
    let mut count = 0;
    while offset < data.len() {
        if data[offset] == 0 {
            break;
        }
        let Some(end) = data[offset..].iter().position(|v| *v == 0) else {
            break;
        };
        let kind = &data[offset..offset + end];
        offset += end + 1;
        let Ok(length) = be_u32(data, offset) else {
            break;
        };
        offset += 4;
        let Some(body) = data.get(offset..offset.saturating_add(length as usize)) else {
            break;
        };
        offset += length as usize;
        count += 1;
        if count > MAX_ITEMS {
            return Err(Error::ResourceLimit);
        }
        match kind {
            b"CUE" => result.cue_points.push(parse_cue(body)?),
            b"LOOP" => result.loops.push(parse_loop(body)?),
            b"FLIP" => result.flips.push(parse_flip(body)?),
            b"COLOR" => result.track_color = Some(color(body, 1, None)?),
            b"BPMLOCK" => result.bpm_lock = Some(byte(body, 0)? != 0),
            _ => {}
        }
    }
    result.cue_points.sort_by_key(|v| v.index);
    result.loops.sort_by_key(|v| v.index);
    result.flips.sort_by_key(|v| v.index);
    Ok(result)
}
pub fn parse_markers2_from_base64(text: &str) -> Result<SeratoMarkers2> {
    parse_markers2(&decode_with_linebreaks(text)?)
}
pub fn parse_markers2_from_geob(data: &[u8]) -> Result<SeratoMarkers2> {
    check_size(data)?;
    if data.len() < 2 {
        return Ok(SeratoMarkers2::default());
    }
    let start = if data.starts_with(&[1, 1]) { 2 } else { 0 };
    let mut end = data.len();
    while end > start && data[end - 1] == 0 {
        end -= 1;
    }
    // Node's ASCII decoder masks the high bit.
    let text = data[start..end]
        .iter()
        .map(|v| char::from(v & 0x7f))
        .collect::<String>();
    parse_markers2_from_base64(&text)
}
pub fn parse_beatgrid(data: &[u8]) -> Result<SeratoBeatgrid> {
    check_size(data)?;
    let mut result = SeratoBeatgrid::default();
    if data.len() < 6 {
        return Ok(result);
    }
    let count = be_u32(data, 2)? as usize;
    if count > MAX_ITEMS {
        return Err(Error::ResourceLimit);
    }
    if count > (data.len() - 6) / 8 {
        return Ok(result);
    }
    for i in 0..count {
        let offset = 6 + i * 8;
        result.markers.push(SeratoBeatgridMarker {
            position: be_f32(data, offset)?,
            bpm: if i + 1 == count {
                Some(be_f32(data, offset + 4)?)
            } else {
                None
            },
            beats_to_next: if i + 1 == count {
                None
            } else {
                Some(be_u32(data, offset + 4)?)
            },
        })
    }
    Ok(result)
}
pub fn parse_beatgrid_from_base64(text: &str) -> Result<SeratoBeatgrid> {
    parse_beatgrid(&decode_with_linebreaks(text)?)
}
pub fn get_bpm_from_beatgrid(grid: &SeratoBeatgrid) -> Option<f64> {
    grid.markers.last()?.bpm
}
pub fn is_dynamic_beatgrid(grid: &SeratoBeatgrid) -> bool {
    grid.markers.len() > 1
}
pub fn parse_autotags(data: &[u8]) -> Result<SeratoAutotags> {
    check_size(data)?;
    let start = if data.starts_with(&[1, 1]) { 2 } else { 0 };
    let mut offset = start;
    let mut values = [0.0; 3];
    for (i, length) in [7, 7, 6].into_iter().enumerate() {
        if let Some(part) = data.get(offset..offset + length) {
            let ascii = part
                .iter()
                .map(|v| char::from(v & 0x7f))
                .collect::<String>();
            values[i] = parse_float(&c_string(ascii.as_bytes())).unwrap_or(0.0);
            offset += length;
        }
    }
    Ok(SeratoAutotags {
        bpm: values[0],
        auto_gain: values[1],
        gain_db: values[2],
    })
}
pub fn parse_autotags_from_base64(text: &str) -> Result<SeratoAutotags> {
    parse_autotags(&decode_with_linebreaks(text)?)
}
