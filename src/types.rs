use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}
pub trait Logger: Send + Sync {
    fn log(&self, _level: LogLevel, _message: &str) {}
    fn trace(&self, message: &str) {
        self.log(LogLevel::Trace, message);
    }
    fn debug(&self, message: &str) {
        self.log(LogLevel::Debug, message);
    }
    fn info(&self, message: &str) {
        self.log(LogLevel::Info, message);
    }
    fn warn(&self, message: &str) {
        self.log(LogLevel::Warn, message);
    }
    fn error(&self, message: &str) {
        self.log(LogLevel::Error, message);
    }
}
pub struct NoopLogger;
impl Logger for NoopLogger {}
pub fn noop_logger() -> Arc<dyn Logger> {
    Arc::new(NoopLogger)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeratoVersion {
    V3,
    V4,
    #[default]
    Unknown,
}
/// Epoch milliseconds in Rust, ISO8601 Date JSON compatible with the TS API.
pub mod optional_date {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &Option<i64>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(ms) => {
                let date = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(*ms)
                    .ok_or_else(|| serde::ser::Error::custom("Date outside supported range"))?;
                serializer.serialize_str(&date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            }
            None => serializer.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<i64>, D::Error> {
        let text = Option::<String>::deserialize(deserializer)?;
        text.map(|s| {
            chrono::DateTime::parse_from_rfc3339(&s)
                .map(|d| d.timestamp_millis())
                .map_err(serde::de::Error::custom)
        })
        .transpose()
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoHistorySong {
    pub title: String,
    pub artist: String,
    pub file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    #[serde(
        default,
        with = "optional_date",
        skip_serializing_if = "Option::is_none"
    )]
    pub start_time: Option<i64>,
    #[serde(
        default,
        with = "optional_date",
        skip_serializing_if = "Option::is_none"
    )]
    pub play_time: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub played: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deck: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streaming_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streaming_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SeratoSession {
    pub date: String,
    pub index: i64,
    pub songs: Vec<SeratoHistorySong>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SeratoColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a: Option<u8>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SeratoCuePoint {
    pub index: u8,
    pub position: u32,
    pub color: SeratoColor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoLoop {
    pub index: u8,
    pub start_position: u32,
    pub end_position: u32,
    pub color: SeratoColor,
    pub locked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeratoFlipActionType {
    Jump,
    Censor,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoFlipAction {
    #[serde(rename = "type")]
    pub action_type: SeratoFlipActionType,
    pub source_position: f64,
    pub target_position: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_factor: Option<f64>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SeratoFlip {
    pub index: u8,
    pub enabled: bool,
    pub name: String,
    #[serde(rename = "loop")]
    pub looping: bool,
    pub actions: Vec<SeratoFlipAction>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoBeatgridMarker {
    pub position: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beats_to_next: Option<u32>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SeratoBeatgrid {
    pub markers: Vec<SeratoBeatgridMarker>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoAutotags {
    pub bpm: f64,
    pub auto_gain: f64,
    pub gain_db: f64,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoMarkers2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_color: Option<SeratoColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm_lock: Option<bool>,
    pub cue_points: Vec<SeratoCuePoint>,
    pub loops: Vec<SeratoLoop>,
    pub flips: Vec<SeratoFlip>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SeratoTrackMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markers: Option<SeratoMarkers2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beatgrid: Option<SeratoBeatgrid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autotags: Option<SeratoAutotags>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<Vec<u8>>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoCrate {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub track_paths: Vec<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoDatabaseTrack {
    pub file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beatgrid_locked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<bool>,
    #[serde(
        default,
        with = "optional_date",
        skip_serializing_if = "Option::is_none"
    )]
    pub date_added: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grouping: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub composer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<f64>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeratoDatabaseStats {
    pub total_tracks: usize,
    pub missing_tracks: usize,
    pub locked_beatgrids: usize,
    pub genres: Vec<String>,
    pub artists: usize,
}
