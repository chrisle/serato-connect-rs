//! Native Serato Remote protocol. Constructing options/state opens no network.
//! Only explicit server/client start and publication advertise or bind sockets.
pub mod client;
pub mod framing;
pub mod mdns;
pub mod osc;
pub mod resolver;
pub mod server;
pub use client::*;
pub use framing::*;
pub use mdns::*;
pub use osc::*;
pub use resolver::*;
pub use server::*;
use std::fmt;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteError {
    InvalidData,
    ResourceLimit,
    Io,
    AlreadyRunning,
    Stopped,
    Deadline,
    Publication,
}
impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidData => "Invalid Serato Remote protocol data",
            Self::ResourceLimit => "Serato Remote resource limit reached",
            Self::Io => "Serato Remote transport failed",
            Self::AlreadyRunning => "Serato Remote already running",
            Self::Stopped => "Serato Remote stopped",
            Self::Deadline => "Serato Remote deadline exceeded",
            Self::Publication => "Serato Remote discovery publication failed",
        })
    }
}
impl std::error::Error for RemoteError {}
pub type RemoteResult<T> = std::result::Result<T, RemoteError>;
