use serato_connect_rs::*;
use serde_json::{json, Value};
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};
fn context(home: &Path) -> DiscoveryContext {
    DiscoveryContext {
        platform: Platform::MacOs,
        home: home.into(),
        local_app_data: None,
        music_directory: None,
    }
}
fn v3_fixture(root: &Path) {
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
}
fn options(root: &Path, version: SeratoVersion) -> SeratoConnectOptions {
    SeratoConnectOptions {
        serato_path: Some(root.into()),
        force_version: Some(version),
        discovery: Some(context(root)),
        poll_interval: Duration::from_millis(15),
        ..Default::default()
    }
}
fn next_event(connect: &SeratoConnect) -> SeratoEvent {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        assert!(Instant::now() < until, "history event fixture deadline");
        match connect.events().recv_timeout(Duration::from_millis(100)) {
            Ok(SeratoEvent::Poll) => {}
            Ok(event) => return event,
            Err(_) => {}
        }
    }
}
fn normalized(event: SeratoEvent) -> Value {
    let mut value = match event {
        SeratoEvent::Ready(payload) => json!({"type":"ready","payload":payload}),
        SeratoEvent::DeckChange(payload) => json!({"type":"deckChange","payload":payload}),
        SeratoEvent::History(payload) => json!({"type":"history","payload":payload}),
        SeratoEvent::Session(payload) => json!({"type":"session","payload":payload}),
        event => panic!("unexpected fixture event {event:?}"),
    };
    if value["payload"].get("seratoPath").is_some() {
        value["payload"]["seratoPath"] = json!("synthetic")
    }
    if value["payload"].get("session").is_some() {
        value["payload"]["session"]["date"] = json!("DATE");
    }
    value
}
fn canonical(value: Value) -> Value {
    match value {
        Value::Number(v) => json!(v.as_f64().unwrap()),
        Value::Array(v) => Value::Array(v.into_iter().map(canonical).collect()),
        Value::Object(v) => Value::Object(v.into_iter().map(|(k, v)| (k, canonical(v))).collect()),
        v => v,
    }
}
#[test]
fn native_v3_actor_seed_incremental_played_transition_and_session_order_match_actual_ts() {
    let dir = tempfile::tempdir().unwrap();
    v3_fixture(dir.path());
    let mut connect = SeratoConnect::new(options(dir.path(), SeratoVersion::V3));
    assert!(!connect.running());
    connect.start().unwrap();
    let mut observed = vec![
        normalized(next_event(&connect)),
        normalized(next_event(&connect)),
    ];
    assert_eq!(connect.version(), SeratoVersion::V3);
    assert_eq!(connect.get_deck_track(1).unwrap().played, Some(false));
    assert!(connect.get_deck_track(2).is_none());
    thread::sleep(Duration::from_millis(2));
    std::fs::write(
        dir.path().join("History/Sessions/2.session"),
        include_bytes!("../tools/runtime/updated.bin"),
    )
    .unwrap();
    for _ in 0..3 {
        observed.push(normalized(next_event(&connect)));
    }
    assert_eq!(connect.get_deck_track(1).unwrap().played, Some(true));
    assert_eq!(connect.get_now_playing().unwrap().title, "New Live");
    std::fs::write(
        dir.path().join("History/Sessions/3.session"),
        include_bytes!("../tools/runtime/new_session.bin"),
    )
    .unwrap();
    for _ in 0..5 {
        observed.push(normalized(next_event(&connect)));
    }
    let expected: Value =
        serde_json::from_str(include_str!("../tools/runtime/expected.json")).unwrap();
    assert_eq!(
        canonical(json!(observed)),
        canonical(expected["v3Lifecycle"].clone())
    );
    assert!(connect.get_deck_track(1).is_none());
    assert!(connect.get_deck_track(2).is_none());
    assert_eq!(connect.get_deck_track(3).unwrap().title, "New Session");
    connect.stop();
    assert!(!connect.running());
    assert!(connect.events().try_recv().is_err());
    assert!(connect.get_all_deck_states().iter().all(Option::is_none));
    connect.start().unwrap();
    assert!(matches!(next_event(&connect), SeratoEvent::Ready(_)));
    connect.stop();
}
#[test]
fn native_v4_custom_path_readonly_wal_and_in_place_played_updates_work_without_count_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("master.sqlite");
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer
        .execute_batch(include_str!("../tools/runtime/schema.sql"))
        .unwrap();
    writer
        .execute_batch("PRAGMA journal_mode=WAL;PRAGMA wal_autocheckpoint=0")
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut connect = SeratoConnect::new(options(dir.path(), SeratoVersion::V4));
    connect.start().unwrap();
    assert!(matches!(next_event(&connect), SeratoEvent::Ready(_)));
    for _ in 0..2 {
        assert!(matches!(next_event(&connect), SeratoEvent::DeckChange(_)));
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
    writer
        .execute_batch("UPDATE history_entry SET played=1 WHERE id=2")
        .unwrap();
    let event = next_event(&connect);
    assert!(
        matches!(event,SeratoEvent::DeckChange(payload) if payload.deck_id==1&&payload.track.as_ref().unwrap().played==Some(true))
    );
    writer.execute_batch("BEGIN;INSERT INTO history_entry VALUES(6,2,'Uncommitted','Synthetic Artist','1',1700007000,0,1,132,NULL,'Synthetic/one.mp3')").unwrap();
    for _ in 0..3 {
        assert!(matches!(
            connect
                .events()
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            SeratoEvent::Poll
        ));
    }
    writer.execute_batch("ROLLBACK").unwrap();
    writer.execute_batch("INSERT INTO history_entry VALUES(6,2,'Committed','Synthetic Artist','1',1700007000,0,1,132,NULL,'Synthetic/one.mp3')").unwrap();
    assert!(
        matches!(next_event(&connect),SeratoEvent::DeckChange(payload)if payload.track.as_ref().unwrap().title=="Committed")
    );
    assert!(
        matches!(next_event(&connect),SeratoEvent::History(payload) if payload.count==1&&payload.tracks[0].title=="Committed")
    );
    connect.stop();
    assert!(!connect.running());
    assert!(connect.events().try_recv().is_err());
}
#[test]
fn injected_auto_detection_finds_v4_and_custom_paths_without_resolving_personal_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let context = context(dir.path());
    let v4 = context.v4_path().unwrap();
    std::fs::create_dir_all(&v4).unwrap();
    let db = rusqlite::Connection::open(v4.join("master.sqlite")).unwrap();
    db.execute_batch(include_str!("../tools/runtime/schema.sql"))
        .unwrap();
    drop(db);
    assert_eq!(
        detect_serato_version_in(None, &context).unwrap(),
        SeratoVersion::V4
    );
    assert_eq!(
        detect_serato_installation_in(None, &[], &context)
            .unwrap()
            .path,
        v4
    );
    let mut connect = SeratoConnect::new(SeratoConnectOptions {
        discovery: Some(context.clone()),
        poll_interval: Duration::from_millis(20),
        ..Default::default()
    });
    connect.start().unwrap();
    assert_eq!(connect.version(), SeratoVersion::V4);
    connect.stop();
    let alternate = dir.path().join("explicit-v4");
    std::fs::create_dir(&alternate).unwrap();
    std::fs::copy(v4.join("master.sqlite"), alternate.join("master.sqlite")).unwrap();
    let mut connect = SeratoConnect::new(SeratoConnectOptions {
        library_path: Some(alternate),
        discovery: Some(context.clone()),
        poll_interval: Duration::from_millis(20),
        ..Default::default()
    });
    connect.start().unwrap();
    assert_eq!(connect.version(), SeratoVersion::V4);
    connect.stop();
    let redirected = dir.path().join("redirected/_Serato_");
    v3_fixture(&redirected);
    assert_eq!(
        detect_serato_installation_in(None, std::slice::from_ref(&redirected), &context)
            .unwrap()
            .version,
        SeratoVersion::V4
    );
    assert_eq!(
        detect_serato_version_in(Some(&redirected), &context).unwrap(),
        SeratoVersion::V3
    );
}
#[test]
fn history_failed_start_retry_queue_saturation_and_drop_release_owned_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut connect = SeratoConnect::new(options(dir.path(), SeratoVersion::V3));
    assert!(connect.start().is_err());
    assert!(!connect.running());
    v3_fixture(dir.path());
    connect.start().unwrap();
    assert!(connect.set_poll_interval(Duration::ZERO).is_err());
    connect.set_poll_interval(Duration::from_millis(1)).unwrap();
    assert_eq!(connect.poll_interval(), Duration::from_millis(1));
    let until = Instant::now() + Duration::from_secs(3);
    while connect.running() {
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(connect.last_error(), Some(Error::ResourceLimit));
    connect.start().unwrap();
    assert!(connect.running());
    let before = Instant::now();
    drop(connect);
    assert!(before.elapsed() < Duration::from_secs(1));
    let missing = dir.path().join("does-not-exist");
    let mut connect = SeratoConnect::new(options(&missing, SeratoVersion::V4));
    assert!(connect.start().is_err());
    assert!(!missing.exists());
}

#[cfg(unix)]
#[test]
fn v4_fifo_database_is_rejected_without_blocking_sqlite_open() {
    // A regression must fail with a killed/reaped child, not hang the test
    // runner on SQLite's native blocking open of the owned synthetic FIFO.
    if let Some(path) = std::env::var_os("SERATO_V4_FIFO_FIXTURE") {
        let path = std::path::PathBuf::from(path);
        assert!(matches!(
            get_latest_session_songs_v4(Some(&path)),
            Err(Error::InvalidData)
        ));
        assert!(matches!(
            get_latest_session_info_v4(Some(&path)),
            Err(Error::InvalidData)
        ));
        assert!(matches!(
            get_serato_v4_sessions(Some(&path)),
            Err(Error::InvalidData)
        ));
        return;
    }
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("master.sqlite");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "v4_fifo_database_is_rejected_without_blocking_sqlite_open",
            "--nocapture",
        ])
        .env("SERATO_V4_FIFO_FIXTURE", dir.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "FIFO child rejected fixture incorrectly");
            break;
        }
        if Instant::now() >= until {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("SQLite FIFO preflight blocked beyond the fixture deadline");
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn v4_connection_mapping_limits_fail_explicitly_instead_of_returning_partial_paths() {
    let dir = tempfile::tempdir().unwrap();
    let writer = rusqlite::Connection::open(dir.path().join("master.sqlite")).unwrap();
    writer
        .execute_batch(include_str!("../tools/runtime/schema.sql"))
        .unwrap();
    // Repeated IDs defeat a bound on unique-map length. Count every row, even
    // when the final map would retain only one tiny root mapping.
    writer.execute_batch("DROP TABLE connection; CREATE VIEW connection AS WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<1000001) SELECT 1 AS location_id,'/root.sqlite' AS database_uri FROM seq;").unwrap();
    assert!(matches!(
        get_latest_session_songs_v4(Some(dir.path())),
        Err(Error::ResourceLimit)
    ));
    // A few huge repeated URIs must also be bounded independently of row count
    // or the output map's one-byte '/' values.
    writer.execute_batch("DROP VIEW connection; CREATE VIEW connection AS SELECT 1 AS location_id,printf('%.*c',9*1024*1024,'x') || '/root.sqlite' AS database_uri UNION ALL SELECT 1,printf('%.*c',9*1024*1024,'x') || '/root.sqlite';").unwrap();
    assert!(matches!(
        get_latest_session_songs_v4(Some(dir.path())),
        Err(Error::ResourceLimit)
    ));
    assert!(matches!(
        get_serato_v4_sessions(Some(dir.path())),
        Err(Error::ResourceLimit)
    ));
    // Optional lookup absence retains the source fallback, not a resource error.
    writer.execute_batch("DROP VIEW connection").unwrap();
    assert_eq!(
        get_latest_session_songs_v4(Some(dir.path())).unwrap().len(),
        4
    );
}

#[test]
fn stopping_v4_actor_cancels_large_mapping_scan_without_publishing_partial_lookup_data() {
    let dir = tempfile::tempdir().unwrap();
    let writer = rusqlite::Connection::open(dir.path().join("master.sqlite")).unwrap();
    writer
        .execute_batch(include_str!("../tools/runtime/schema.sql"))
        .unwrap();
    let mut connect = SeratoConnect::new(options(dir.path(), SeratoVersion::V4));
    connect.start().unwrap();
    assert!(matches!(next_event(&connect), SeratoEvent::Ready(_)));
    for _ in 0..2 {
        assert!(matches!(next_event(&connect), SeratoEvent::DeckChange(_)));
    }
    writer.execute_batch("DROP TABLE connection; CREATE VIEW connection AS WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<10000000) SELECT 1 AS location_id,'/root.sqlite' AS database_uri FROM seq;").unwrap();
    while !matches!(
        connect
            .events()
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        SeratoEvent::Poll
    ) {}
    let before = Instant::now();
    connect.stop();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(!connect.running());
    assert!(connect.events().try_recv().is_err());
}
