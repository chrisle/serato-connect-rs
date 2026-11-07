// Copyright (c) 2025 Jan Holthuis
// Copyright (c) 2026 Chris Le
// SPDX-License-Identifier: MPL-2.0
// This Source Code Form is subject to the Mozilla Public License, v. 2.0.
// See LICENSES/MPL-2.0.txt or https://mozilla.org/MPL/2.0/.
//! Serato32 and Serato's unpadded, 72-column base64 representation.
//! Format provenance: original MIT serato-connect headers credit Holzhaus
//! serato-tags/triseratops (MPL-2.0 format references); notices retained in tools/reference.
use crate::{
    util::{check_size, js_whitespace},
    Error, Result, SeratoColor,
};
use base64::Engine;
pub fn decode_serato32(a: u8, b: u8, c: u8, d: u8) -> [u8; 3] {
    [
        ((b & 0x7f) >> 2) | ((a & 7) << 5),
        ((c & 0x7f) >> 1) | ((b & 3) << 6),
        (d & 0x7f) | ((c & 1) << 7),
    ]
}
pub fn encode_serato32(a: u8, b: u8, c: u8) -> [u8; 4] {
    [
        a >> 5,
        ((b >> 6) | (a << 2)) & 0x7f,
        ((c >> 7) | (b << 1)) & 0x7f,
        c & 0x7f,
    ]
}
pub fn decode_serato32_buffer(data: &[u8]) -> Result<Vec<u8>> {
    check_size(data)?;
    if !data.len().is_multiple_of(4) {
        return Err(Error::InvalidData);
    }
    Ok(data
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|v| decode_serato32(v[0], v[1], v[2], v[3]))
        .collect())
}
pub fn encode_serato32_buffer(data: &[u8]) -> Result<Vec<u8>> {
    check_size(data)?;
    if !data.len().is_multiple_of(3) {
        return Err(Error::InvalidData);
    }
    Ok(data
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|v| encode_serato32(v[0], v[1], v[2]))
        .collect())
}
pub fn decode_serato32_u32(data: &[u8], offset: usize) -> Result<u32> {
    let v = data
        .get(offset..offset.checked_add(4).ok_or(Error::ResourceLimit)?)
        .ok_or(Error::InvalidData)?;
    let [a, b, c] = decode_serato32(v[0], v[1], v[2], v[3]);
    Ok(u32::from_be_bytes([0, a, b, c]))
}
pub fn encode_serato32_u32(value: u32) -> [u8; 4] {
    let v = value.to_be_bytes();
    encode_serato32(v[1], v[2], v[3])
}
pub fn decode_serato32_color(data: &[u8], offset: usize) -> Result<SeratoColor> {
    let v = decode_serato32_u32(data, offset)?.to_be_bytes();
    Ok(SeratoColor {
        r: v[1],
        g: v[2],
        b: v[3],
        a: None,
    })
}
pub fn encode_serato32_color(color: &SeratoColor) -> [u8; 4] {
    encode_serato32(color.r, color.g, color.b)
}
pub fn encode_with_linebreaks(data: &[u8]) -> Result<String> {
    check_size(data)?;
    let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(data);
    Ok(encoded
        .as_bytes()
        .chunks(72)
        .map(|v| std::str::from_utf8(v).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n"))
}
/// Node Buffer-compatible base64 decoding: ignores nonalphabet bytes, accepts
/// URL-safe characters and missing padding, and stops at the first '='.
pub fn decode_with_linebreaks(text: &str) -> Result<Vec<u8>> {
    check_size(text.as_bytes())?;
    let mut clean = Vec::with_capacity(text.len());
    for c in text.chars().filter(|c| !js_whitespace(*c)) {
        match c {
            '=' => break,
            'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '/' => clean.push(c as u8),
            '-' => clean.push(b'+'),
            '_' => clean.push(b'/'),
            _ => {}
        }
    }
    if clean.len() % 4 == 1 {
        clean.pop();
    }
    let engine = base64::engine::general_purpose::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    );
    engine.decode(clean).map_err(|_| Error::InvalidData)
}
pub fn is_serato_base64(text: &str) -> bool {
    let mut any = false;
    for c in text.chars() {
        if c == '\n' {
            continue;
        }
        if !c.is_ascii_alphanumeric() && !matches!(c, '+' | '/') {
            return false;
        }
        any = true;
    }
    any
}
