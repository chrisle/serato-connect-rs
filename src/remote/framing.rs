use super::{decode_osc_packet, encode_osc, OscMessage, RemoteError, RemoteResult};
pub const FRAME_SENTINEL: [u8; 16] = [
    0x4c, 0xaa, 0xc2, 0xae, 0x35, 0xb1, 0xc4, 0x76, 0xdb, 0x5a, 0x64, 0x44, 0x03, 0xbd, 0x41, 0x70,
];
pub fn frame_osc(message: &OscMessage) -> RemoteResult<Vec<u8>> {
    let mut bytes = encode_osc(message)?;
    bytes.extend(FRAME_SENTINEL);
    Ok(bytes)
}
pub struct FrameReader {
    buffer: Vec<u8>,
    max_frame_bytes: usize,
    failed: bool,
}
impl Default for FrameReader {
    fn default() -> Self {
        Self::new(1 << 20)
    }
}
impl FrameReader {
    pub fn new(max_frame_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            max_frame_bytes: max_frame_bytes.min(16 << 20),
            failed: false,
        }
    }
    pub fn pending_bytes(&self) -> usize {
        self.buffer.len()
    }
    pub fn push(&mut self, chunk: &[u8]) -> RemoteResult<Vec<OscMessage>> {
        if self.failed {
            return Err(RemoteError::InvalidData);
        }
        let result = self.consume(chunk);
        if result.is_err() {
            self.failed = true;
            self.buffer.clear()
        }
        result
    }
    fn consume(&mut self, chunk: &[u8]) -> RemoteResult<Vec<OscMessage>> {
        let mut output = Vec::new();
        for block in chunk.chunks(8192) {
            self.buffer.extend_from_slice(block);
            while let Some(end) = self.buffer.windows(16).position(|v| v == FRAME_SENTINEL) {
                if end > self.max_frame_bytes {
                    return Err(RemoteError::ResourceLimit);
                }
                if end > 0 {
                    let messages = decode_osc_packet(&self.buffer[..end])?;
                    if output.len().saturating_add(messages.len()) > 4096 {
                        return Err(RemoteError::ResourceLimit);
                    }
                    output.extend(messages)
                }
                self.buffer.drain(..end + 16);
            }
            // Permit only an actual sentinel prefix beyond the bare-packet
            // maximum, so a max-sized frame works across every TCP split.
            if self.buffer.len() > self.max_frame_bytes {
                let excess = self.buffer.len() - self.max_frame_bytes;
                if excess >= 16
                    || !(excess..16).any(|size| self.buffer.ends_with(&FRAME_SENTINEL[..size]))
                {
                    return Err(RemoteError::ResourceLimit);
                }
            }
        }
        Ok(output)
    }
}
