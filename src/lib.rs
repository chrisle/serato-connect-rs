//! Native Serato history, library, metadata and Remote protocol. Constructors
//! are passive; filesystem/network ownership begins only at explicit calls.
pub mod types;
pub use types::*;
pub mod encoding;
pub mod parsers;
pub mod readers;
pub use encoding::*;
pub use parsers::*;
pub use readers::*;
pub mod history_v3;
pub mod history_v4;
pub mod remote;
mod util;
pub mod watch;
pub use history_v3::*;
pub use history_v4::*;
pub use remote::*;
pub use watch::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Io,
    InvalidData,
    ResourceLimit,
    UnsupportedPlatform,
    UnsupportedFormat,
    AlreadyRunning,
    Stopped,
    ThreadFailed,
    Timeout,
}
pub type Result<T> = std::result::Result<T, Error>;
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Io => "Cannot read Serato data",
            Self::InvalidData => "Invalid Serato data",
            Self::ResourceLimit => "Serato resource limit reached",
            Self::UnsupportedPlatform => "Serato requires macOS or Windows",
            Self::UnsupportedFormat => "Unsupported audio container",
            Self::AlreadyRunning => "Serato connector already running",
            Self::Stopped => "Serato connector stopped",
            Self::ThreadFailed => "Cannot run Serato worker",
            Self::Timeout => "Serato operation timed out",
        })
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}
