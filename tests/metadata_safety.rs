use serato_connect_rs::*;
use std::{
    fs,
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
};
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/metadata")
}
#[test]
fn connector_metadata_facade_is_passive_and_preserves_all_library_queries() {
    let connector = SeratoConnect::new(SeratoConnectOptions {
        serato_path: Some(root()),
        ..Default::default()
    });
    assert!(!connector.running());
    assert_eq!(connector.version(), SeratoVersion::Unknown);
    assert_eq!(connector.get_crates().unwrap().len(), 2);
    assert_eq!(
        connector.get_crate("A").unwrap().unwrap().track_paths.len(),
        2
    );
    assert!(connector.get_crate("missing").unwrap().is_none());
    assert_eq!(
        connector
            .find_crates_for_track("/Drive/Music/Album/two.flac")
            .unwrap(),
        ["A", "B"]
    );
    assert_eq!(connector.get_library_tracks(None).unwrap().len(), 2);
    assert_eq!(
        connector
            .get_library_tracks(Some(LibraryQuery {
                limit: Some(1),
                offset: Some(1)
            }))
            .unwrap()[0]
            .title
            .as_deref(),
        Some("Two")
    );
    assert_eq!(
        connector
            .get_library_tracks(Some(LibraryQuery {
                limit: Some(0),
                offset: None
            }))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        connector
            .get_library_tracks(Some(LibraryQuery {
                limit: None,
                offset: Some(-1)
            }))
            .unwrap()[0]
            .title
            .as_deref(),
        Some("Two")
    );
    assert_eq!(
        connector
            .get_track_by_path("two.flac")
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some("Two")
    );
    assert_eq!(connector.search_library("SEARCH ME").unwrap().len(), 1);
    assert_eq!(connector.get_database_stats().unwrap().total_tracks, 2);
    assert!(connector
        .get_track_metadata(root().join("synthetic.flac"))
        .unwrap()
        .is_some());
    assert!(
        !connector.running(),
        "querying metadata must not implicitly start history/network capture"
    );
}
#[test]
fn malformed_lengths_and_resource_bounds_are_explicit_without_panics() {
    for name in [
        "synthetic-v24.mp3",
        "synthetic-v23.mp3",
        "synthetic-utf16.mp3",
        "synthetic.aiff",
        "synthetic.wav",
        "synthetic-fragmented.ogg",
        "synthetic.flac",
        "synthetic.ogg",
        "synthetic.m4a",
    ] {
        let bytes = fs::read(root().join(name)).unwrap();
        // Exercise every header prefix, plus cuts throughout container metadata.
        for length in (0..bytes.len().min(96)).chain((96..bytes.len()).step_by(19)) {
            let _ = get_track_metadata_from_buffer(&bytes[..length], "application/octet-stream");
        }
    }
    let mut count = vec![1, 1];
    count.extend_from_slice(&1_000_001u32.to_be_bytes());
    assert_eq!(parse_beatgrid(&count), Err(Error::ResourceLimit));
    assert_eq!(
        decode_serato32_u32(&[], usize::MAX),
        Err(Error::ResourceLimit)
    );
    let tag_len = 16 * 1024 * 1024 + 1;
    let header = [
        b'I',
        b'D',
        b'3',
        4,
        0,
        0,
        ((tag_len >> 21) & 127) as u8,
        ((tag_len >> 14) & 127) as u8,
        ((tag_len >> 7) & 127) as u8,
        (tag_len & 127) as u8,
    ];
    assert_eq!(
        get_track_metadata_from_buffer(&header, "audio/mpeg"),
        Err(Error::ResourceLimit)
    );
    let temp = tempfile::tempdir().unwrap();
    let oversized = temp.path().join("oversized.crate");
    let file = fs::File::create(&oversized).unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    assert_eq!(parse_crate(&oversized), Err(Error::ResourceLimit));
    assert!(get_track_metadata(temp.path()).unwrap().is_none());
    assert!(get_track_metadata(temp.path().join("missing-fixture.mp3"))
        .unwrap()
        .is_none());
    assert!(get_library_tracks(temp.path()).unwrap().is_empty());
    assert!(list_crates(temp.path()).unwrap().is_empty());
}
#[test]
fn large_audio_payload_is_seeked_without_loading_or_decoding_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("large-synthetic.m4a");
    let prefix = fs::read(root().join("synthetic.m4a")).unwrap();
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(&prefix).unwrap();
    // An extended-size mdat advertises a 256 MiB sparse payload. The metadata
    // reader must seek past it, not read/cap the complete audio file.
    let size = 256 * 1024 * 1024u64;
    file.write_all(&1u32.to_be_bytes()).unwrap();
    file.write_all(b"mdat").unwrap();
    file.write_all(&size.to_be_bytes()).unwrap();
    file.seek(SeekFrom::Start(prefix.len() as u64 + size - 1))
        .unwrap();
    file.write_all(&[0]).unwrap();
    drop(file);
    let metadata = get_track_metadata(&path).unwrap().unwrap();
    assert_eq!(metadata.markers.unwrap().cue_points.len(), 2);
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        prefix.len() as u64 + size
    );
}
#[cfg(unix)]
#[test]
fn fifo_sources_are_rejected_without_opening_a_blocking_pipe() {
    use std::{
        ffi::CString,
        os::unix::ffi::OsStrExt,
        time::{Duration, Instant},
    };
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("synthetic-pipe");
    let name = CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let before = Instant::now();
    assert_eq!(parse_crate(&path), Err(Error::InvalidData));
    assert!(get_track_metadata(&path).unwrap().is_none());
    assert!(before.elapsed() < Duration::from_secs(1));
}
#[test]
fn v4_history_start_and_stop_never_replace_the_legacy_metadata_root() {
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let music = temp.path().join("SyntheticMusic");
    let legacy = music.join("_Serato_");
    fs::create_dir_all(legacy.join("Subcrates")).unwrap();
    for name in ["database V2", "Subcrates/A.crate", "Subcrates/B.crate"] {
        fs::copy(root().join(name), legacy.join(name)).unwrap();
    }
    let v4 = temp.path().join("SyntheticV4Library");
    fs::create_dir(&v4).unwrap();
    let writer = rusqlite::Connection::open(v4.join("master.sqlite")).unwrap();
    writer
        .execute_batch(include_str!("../tools/runtime/schema.sql"))
        .unwrap();
    drop(writer);
    let assert_queries = |connector: &SeratoConnect| {
        assert_eq!(connector.get_library_tracks(None).unwrap().len(), 2);
        assert_eq!(connector.get_crates().unwrap().len(), 2);
        assert_eq!(
            connector.get_crate("A").unwrap().unwrap().track_paths.len(),
            2
        );
        assert_eq!(
            connector
                .find_crates_for_track("/Drive/Music/Album/two.flac")
                .unwrap(),
            ["A", "B"]
        );
        assert_eq!(connector.search_library("search me").unwrap().len(), 1);
        assert!(connector.get_track_by_path("two.flac").unwrap().is_some());
        assert_eq!(connector.get_database_stats().unwrap().total_tracks, 2);
    };
    // Verify both a caller-supplied legacy root and the injected OS Music root.
    // Every path is synthetic; start performs no native profile discovery.
    for explicit in [true, false] {
        let mut connector = SeratoConnect::new(SeratoConnectOptions {
            serato_path: explicit.then(|| legacy.clone()),
            library_path: Some(v4.clone()),
            force_version: Some(SeratoVersion::V4),
            poll_interval: Duration::from_secs(60),
            discovery: Some(DiscoveryContext {
                platform: Platform::MacOs,
                home: temp.path().join("SyntheticProfile"),
                local_app_data: None,
                music_directory: Some(music.clone()),
            }),
            ..Default::default()
        });
        assert_queries(&connector);
        connector.start().unwrap();
        assert_eq!(connector.version(), SeratoVersion::V4);
        assert!(connector.running());
        assert_queries(&connector);
        connector.stop();
        assert!(!connector.running());
        assert_queries(&connector);
    }
}
