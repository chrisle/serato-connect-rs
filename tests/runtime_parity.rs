use serato_connect_rs::*;
use serde_json::{json, Value};
use std::path::Path;
fn expected() -> Value {
    serde_json::from_str(include_str!("../tools/runtime/expected.json")).unwrap()
}
fn canonical(value: Value) -> Value {
    match value {
        Value::Number(v) => json!(v.as_f64().unwrap()),
        Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
        Value::Object(values) => {
            Value::Object(values.into_iter().map(|(k, v)| (k, canonical(v))).collect())
        }
        v => v,
    }
}
fn equal(actual: impl serde::Serialize, expected: &Value) {
    assert_eq!(
        canonical(serde_json::to_value(actual).unwrap()),
        canonical(expected.clone())
    )
}
fn hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
#[test]
fn generated_v3_binary_queries_match_actual_pinned_typescript() {
    let expected = expected();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("History/Sessions")).unwrap();
    std::fs::write(
        root.join("History/history.database"),
        include_bytes!("../tools/runtime/sessions.bin"),
    )
    .unwrap();
    for id in [1, 2] {
        std::fs::write(
            root.join(format!("History/Sessions/{id}.session")),
            include_bytes!("../tools/runtime/session.bin"),
        )
        .unwrap();
    }
    equal(
        parse_session_bytes(include_bytes!("../tools/runtime/session.bin")).unwrap(),
        &expected["v3Songs"],
    );
    equal(
        get_sessions(root.join("History/history.database")).unwrap(),
        &expected["v3Sessions"],
    );
    equal(get_serato_history(root).unwrap(), &expected["v3History"]);
    assert_eq!(
        get_latest_session_path(root)
            .unwrap()
            .unwrap()
            .file_name()
            .unwrap(),
        expected["latestPath"].as_str().unwrap()
    );
    assert!(get_session_songs(root.join("missing.session")).is_err());
    assert!(parse_session_bytes(&[0, 1, 2]).is_err());
}
#[test]
fn generated_plaintext_v4_sqlite_queries_match_actual_pinned_typescript() {
    let expected = expected();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("master.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(include_str!("../tools/runtime/schema.sql"))
        .unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    equal(
        get_serato_v4_sessions(Some(dir.path())).unwrap(),
        &expected["v4Sessions"],
    );
    equal(
        get_latest_session_songs_v4(Some(dir.path())).unwrap(),
        &expected["v4Songs"],
    );
    equal(
        get_latest_session_info_v4(Some(dir.path())).unwrap(),
        &expected["v4Info"],
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(has_serato_v4_database(Some(dir.path())).unwrap());
    assert_eq!(get_master_database_path(Some(dir.path())).unwrap(), path);
    assert!(get_database_mtime(Some(dir.path())).unwrap() > 0.0);
    for (input, expected) in [
        ("+1abc", Some(1)),
        (" a", None),
        ("a", Some(1)),
        ("1.5", Some(1)),
        ("-1", None),
        ("4", Some(4)),
    ] {
        assert_eq!(parse_deck_value(input), expected)
    }
    let missing = dir.path().join("missing");
    assert!(get_latest_session_songs_v4(Some(&missing))
        .unwrap()
        .is_empty());
    assert!(!missing.exists());
}
#[test]
fn actual_typescript_osc_and_fragmented_frames_match_native_wire_and_decode() {
    let expected = expected();
    let mut reader = FrameReader::default();
    let mut framed = Vec::new();
    for fixture in expected["osc"].as_array().unwrap() {
        let wire = hex(fixture["hex"].as_str().unwrap());
        let decoded = decode_osc(&wire).unwrap();
        equal(&decoded, &fixture["decoded"]);
        assert_eq!(encode_osc(&decoded).unwrap(), wire);
        let frame = hex(fixture["frame"].as_str().unwrap());
        assert_eq!(frame_osc(&decoded).unwrap(), frame);
        for byte in frame {
            framed.extend(reader.push(&[byte]).unwrap());
        }
    }
    equal(framed, &expected["framed"]);
    assert_eq!(reader.pending_bytes(), 0);
    // The packet limit excludes the sentinel, including every partial-sentinel
    // TCP split. The original framer rejected some of these valid splits.
    let message = osc("/Ping", []);
    let frame = frame_osc(&message).unwrap();
    let length = frame.len() - 16;
    for extra in 0..4 {
        for split in 0..frame.len() {
            let mut reader = FrameReader::new(length + extra);
            let mut output = reader.push(&frame[..split]).unwrap();
            output.extend(reader.push(&frame[split..]).unwrap());
            assert_eq!(output, vec![message.clone()]);
        }
    }
    assert!(FrameReader::new(length - 1).push(&frame).is_err());
    let mut bundle = b"#bundle\0".to_vec();
    bundle.extend([0; 8]);
    bundle.extend((length as u32).to_be_bytes());
    bundle.extend(&frame[..length]);
    bundle.extend(FRAME_SENTINEL);
    assert_eq!(FrameReader::default().push(&bundle).unwrap(), vec![message]);
    assert!(decode_osc_packet(b"#bundle\0").is_err());
}
fn normalized(event: SeratoRemoteEvent) -> Value {
    match event {
        SeratoRemoteEvent::DeckChange(payload) => json!({"type":"deckChange","payload":payload}),
        SeratoRemoteEvent::Playhead(payload) => json!({"type":"playhead","payload":payload}),
        SeratoRemoteEvent::LoopChange(payload) => json!({"type":"loopChange","payload":payload}),
        SeratoRemoteEvent::MixerChange(payload) => json!({"type":"mixerChange","payload":payload}),
        _ => panic!("Unexpected runtime fixture event"),
    }
}
#[test]
fn fifty_ms_settled_deck_playhead_loop_mixer_events_match_actual_typescript() {
    let expected = expected();
    let mut state = RemoteState::default();
    let mut events = Vec::new();
    for (at, title, artist, path) in [
        (0, "Old", "Old Artist", "/Synthetic/old.mp3"),
        (100, "New", "New Artist", "/Synthetic/new.mp3"),
    ] {
        for (address, value) in [("Title", title), ("Artist", artist), ("Filepath", path)] {
            events.extend(state.handle_status(
                &osc(
                    format!("/Status/Deck/Song/{address}"),
                    [arg::i(0), arg::s(value)],
                ),
                at,
            ));
        }
        assert!(state.flush(at + 49).is_empty());
        events.extend(state.flush(at + 50));
    }
    for message in [
        osc(
            "/Status/Deck/Playhead",
            [arg::i(0), arg::f(12.5), arg::f(1.0), arg::f(128.0)],
        ),
        osc("/Status/Deck/Loop/AutoLoopOn", [arg::i(0), arg::f(1.0)]),
        osc("/Status/Deck/Loop/BeatLength", [arg::i(0), arg::f(4.0)]),
        osc("/Status/Deck/Loop/LoopRollOn", [arg::i(0), arg::i(0)]),
        osc("/Status/Video/Mixer/Crossfader", [arg::f(0.25)]),
        osc(
            "/Status/Video/Deck/Mixer/Upfader",
            [arg::i(0), arg::f(0.75)],
        ),
        osc("/Status/Deck/Song/Valid", [arg::i(0), arg::f(0.0)]),
    ] {
        events.extend(state.handle_status(&message, 200));
    }
    equal(
        events.into_iter().map(normalized).collect::<Vec<_>>(),
        &expected["remote"],
    );
    equal(state.get_mixer(), &expected["mixer"]);
    equal(state.get_deck_loop(1), &expected["loop"]);
    equal(state.get_deck_track(1), &expected["track"]);
    assert!(state.get_deck_track(0).is_none());
    assert!(state.get_deck_track(5).is_none());
    state.reset();
    assert_eq!(state.get_mixer().upfaders, [None; 5]);
}
#[test]
fn explicit_discovery_contexts_resolve_os_paths_without_accessing_real_profiles() {
    let context = DiscoveryContext {
        platform: Platform::Windows,
        home: Path::new("/Synthetic/Profile").into(),
        local_app_data: Some("/Redirected/Local".into()),
        music_directory: Some("/Redirected/Music".into()),
    };
    assert_eq!(
        context.v3_path().unwrap(),
        Path::new("/Redirected/Music/_Serato_")
    );
    assert_eq!(
        context.v4_path().unwrap(),
        Path::new("/Redirected/Local/Serato/Library")
    );
    let unsupported = DiscoveryContext {
        platform: Platform::Unsupported,
        ..context
    };
    assert!(unsupported.v3_path().is_err());
}

#[test]
fn synthetic_runtime_fixture_hashes_and_readonly_inputs_remain_unchanged() {
    use sha2::{Digest, Sha256};
    let provenance: Value =
        serde_json::from_str(include_str!("../tools/runtime/PROVENANCE.json")).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/runtime");
    for (name, expected) in provenance["sha256"].as_object().unwrap() {
        let path = root.join(name);
        let before = std::fs::read(&path).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&before)),
            expected.as_str().unwrap()
        );
        if name == "session.bin" {
            get_session_songs(&path).unwrap();
        }
        if name == "sessions.bin" {
            get_sessions(&path).unwrap();
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
