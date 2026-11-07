use super::{RemoteError, RemoteResult};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum OscArg {
    #[serde(rename = "i")]
    Int(i32),
    #[serde(rename = "f")]
    Float(#[serde(serialize_with = "serialize_float")] f32),
    #[serde(rename = "s")]
    String(String),
    #[serde(rename = "b")]
    Blob(Vec<u8>),
}
fn serialize_float<S: serde::Serializer>(
    value: &f32,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_f64(f64::from(*value))
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OscMessage {
    pub address: String,
    pub args: Vec<OscArg>,
}
pub fn osc(address: impl Into<String>, args: impl IntoIterator<Item = OscArg>) -> OscMessage {
    OscMessage {
        address: address.into(),
        args: args.into_iter().collect(),
    }
}
pub mod arg {
    use super::OscArg;
    pub fn i(value: i32) -> OscArg {
        OscArg::Int(value)
    }
    pub fn f(value: f32) -> OscArg {
        OscArg::Float(value)
    }
    pub fn s(value: impl Into<String>) -> OscArg {
        OscArg::String(value.into())
    }
    pub fn b(value: impl Into<Vec<u8>>) -> OscArg {
        OscArg::Blob(value.into())
    }
}
const MAX_PACKET: usize = 16 * 1024 * 1024;
fn padded(length: usize) -> RemoteResult<usize> {
    length
        .checked_add(3)
        .map(|v| v & !3)
        .ok_or(RemoteError::ResourceLimit)
}
fn string(out: &mut Vec<u8>, value: &str) -> RemoteResult<()> {
    if value.contains('\0') {
        return Err(RemoteError::InvalidData);
    }
    let size = padded(
        value
            .len()
            .checked_add(1)
            .ok_or(RemoteError::ResourceLimit)?,
    )?;
    if out.len().saturating_add(size) > MAX_PACKET {
        return Err(RemoteError::ResourceLimit);
    }
    out.extend_from_slice(value.as_bytes());
    out.resize(out.len() + size - value.len(), 0);
    Ok(())
}
pub fn encode_osc(message: &OscMessage) -> RemoteResult<Vec<u8>> {
    let mut out = Vec::new();
    string(&mut out, &message.address)?;
    let mut tags = String::from(",");
    for value in &message.args {
        tags.push(match value {
            OscArg::Int(_) => 'i',
            OscArg::Float(_) => 'f',
            OscArg::String(_) => 's',
            OscArg::Blob(_) => 'b',
        });
    }
    string(&mut out, &tags)?;
    for value in &message.args {
        match value {
            OscArg::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
            OscArg::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
            OscArg::String(v) => string(&mut out, v)?,
            OscArg::Blob(v) => {
                let len = u32::try_from(v.len()).map_err(|_| RemoteError::ResourceLimit)?;
                if out.len().saturating_add(padded(v.len())?).saturating_add(4) > MAX_PACKET {
                    return Err(RemoteError::ResourceLimit);
                }
                out.extend_from_slice(&len.to_be_bytes());
                out.extend_from_slice(v);
                out.resize(out.len() + padded(v.len())? - v.len(), 0);
            }
        }
        if out.len() > MAX_PACKET {
            return Err(RemoteError::ResourceLimit);
        }
    }
    Ok(out)
}
fn read_string(bytes: &[u8], at: &mut usize) -> RemoteResult<String> {
    let tail = bytes.get(*at..).ok_or(RemoteError::InvalidData)?;
    let end = tail
        .iter()
        .position(|v| *v == 0)
        .ok_or(RemoteError::InvalidData)?;
    let value = String::from_utf8_lossy(&tail[..end]).into_owned();
    *at = at
        .checked_add(padded(end + 1)?)
        .ok_or(RemoteError::ResourceLimit)?;
    if *at > bytes.len() {
        return Err(RemoteError::InvalidData);
    }
    Ok(value)
}
fn word(bytes: &[u8], at: &mut usize) -> RemoteResult<[u8; 4]> {
    let value = bytes
        .get(*at..at.saturating_add(4))
        .ok_or(RemoteError::InvalidData)?
        .try_into()
        .map_err(|_| RemoteError::InvalidData)?;
    *at += 4;
    Ok(value)
}
pub fn decode_osc(bytes: &[u8]) -> RemoteResult<OscMessage> {
    if bytes.len() > MAX_PACKET {
        return Err(RemoteError::ResourceLimit);
    }
    let mut at = 0;
    let address = read_string(bytes, &mut at)?;
    if at == bytes.len() {
        return Ok(osc(address, []));
    }
    let tags = read_string(bytes, &mut at)?;
    if tags.is_empty() {
        return Ok(osc(address, []));
    }
    let tags = tags.strip_prefix(',').ok_or(RemoteError::InvalidData)?;
    if tags.len() > 4096 {
        return Err(RemoteError::ResourceLimit);
    }
    let mut args = Vec::new();
    for tag in tags.bytes() {
        args.push(match tag {
            b'i' => OscArg::Int(i32::from_be_bytes(word(bytes, &mut at)?)),
            b'f' => OscArg::Float(f32::from_be_bytes(word(bytes, &mut at)?)),
            b's' => OscArg::String(read_string(bytes, &mut at)?),
            b'b' => {
                let size = u32::from_be_bytes(word(bytes, &mut at)?) as usize;
                let end = at.checked_add(size).ok_or(RemoteError::ResourceLimit)?;
                let value = bytes.get(at..end).ok_or(RemoteError::InvalidData)?.to_vec();
                at = at
                    .checked_add(padded(size)?)
                    .ok_or(RemoteError::ResourceLimit)?;
                if at > bytes.len() {
                    return Err(RemoteError::InvalidData);
                }
                OscArg::Blob(value)
            }
            _ => return Err(RemoteError::InvalidData),
        });
    }
    Ok(OscMessage { address, args })
}
pub fn decode_osc_packet(bytes: &[u8]) -> RemoteResult<Vec<OscMessage>> {
    fn visit(bytes: &[u8], depth: usize, out: &mut Vec<OscMessage>) -> RemoteResult<()> {
        if depth > 32 || out.len() >= 4096 || bytes.len() > MAX_PACKET {
            return Err(RemoteError::ResourceLimit);
        }
        if bytes.starts_with(b"#bundle\0") {
            if bytes.len() < 16 {
                return Err(RemoteError::InvalidData);
            }
            let mut at = 16;
            while at < bytes.len() {
                let size = u32::from_be_bytes(word(bytes, &mut at)?) as usize;
                if size == 0 {
                    continue;
                }
                let end = at.checked_add(size).ok_or(RemoteError::ResourceLimit)?;
                visit(
                    bytes.get(at..end).ok_or(RemoteError::InvalidData)?,
                    depth + 1,
                    out,
                )?;
                at = end;
            }
        } else {
            out.push(decode_osc(bytes)?)
        }
        Ok(())
    }
    let mut out = Vec::new();
    visit(bytes, 0, &mut out)?;
    Ok(out)
}
