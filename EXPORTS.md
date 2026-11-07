# Original public exports and Rust equivalents

Original: chrisle/serato-connect1.4.6 at
`8ce4aaf7642968fbde21ad413eff5b89db4debf4`. This table covers both
`src/index.ts` and `src/remote/index.ts`; no public library area is excluded.
The original npm manifest exports only the root package.

| Original runtime API | Rust API |
|---|---|
| SeratoConnect | SeratoConnect; new/start/stop/events/running/version/last_error |
| getDefaultSeratoPath / detectSeratoInstallation / detectSeratoVersion | get_default_serato_path / detect_serato_installation / detect_serato_version; explicit DiscoveryContext variants also available |
| getDefaultSeratoV4LibraryPath / getMasterDatabasePath / hasSeratoV4Database | get_default_serato_v4_library_path / get_master_database_path / has_serato_v4_database |
| getSeratoV4Sessions / getLatestSessionSongsV4 / getLatestSessionInfoV4 / getDatabaseMtime | get_serato_v4_sessions / get_latest_session_songs_v4 / get_latest_session_info_v4 / get_database_mtime |
| noopLogger | noop_logger / NoopLogger |
| getSeratoHistory / getSessions / getSessionSongs / getLatestSessionPath / CHUNK_TAGS | get_serato_history / get_sessions / get_session_songs / get_latest_session_path / CHUNK_TAGS |
| decodeSerato32 / encodeSerato32 | decode_serato32 / encode_serato32 |
| decodeSerato32Buffer / encodeSerato32Buffer | decode_serato32_buffer / encode_serato32_buffer |
| decodeSerato32U32 / encodeSerato32U32 | decode_serato32_u32 / encode_serato32_u32 |
| decodeSerato32Color / encodeSerato32Color | decode_serato32_color / encode_serato32_color |
| encodeWithLinebreaks / decodeWithLinebreaks / isSeratoBase64 | encode_with_linebreaks / decode_with_linebreaks / is_serato_base64 |
| parseMarkers2 / parseMarkers2FromBase64 / parseMarkers2FromGeob | parse_markers2 / parse_markers2_from_base64 / parse_markers2_from_geob |
| parseBeatgrid / parseBeatgridFromBase64 / getBpmFromBeatgrid / isDynamicBeatgrid | parse_beatgrid / parse_beatgrid_from_base64 / get_bpm_from_beatgrid / is_dynamic_beatgrid |
| parseAutotags / parseAutotagsFromBase64 | parse_autotags / parse_autotags_from_base64 |
| parseCrate / parseCrateSync / listCrates / getAllCrates / findCratesForTrack | parse_crate / parse_crate_sync / list_crates / get_all_crates / find_crates_for_track |
| parseDatabase / parseDatabaseSync / getLibraryTracks / getTrackByPath / searchLibrary / getDatabaseStats | parse_database / parse_database_sync / get_library_tracks / get_track_by_path / search_library / get_database_stats |
| getTrackMetadata / getTrackMetadataFromBuffer | get_track_metadata / get_track_metadata_from_buffer |
| SeratoRemoteClient / RemoteServer / RemoteSession | same Rust struct names; explicit start/stop or close and typed event receivers |
| publishSeratoRemote / buildInstanceName / SERATO_REMOTE_SERVICE_TYPE | publish_serato_remote / build_instance_name / SERATO_REMOTE_SERVICE_TYPE |
| encodeOsc / decodeOsc / osc / arg | encode_osc / decode_osc / osc / arg::{i,f,s,b} |
| frameOsc / FrameReader / FRAME_SENTINEL | frame_osc / FrameReader::new/push/pending_bytes / FRAME_SENTINEL |
| NUM_REMOTE_DECKS / DEFAULT_SUBSCRIPTION_TOPICS | same constant names |

## Connector forwarding methods

`getDeckTrack`, `getAllDeckStates`, `getNowPlaying`, `setPollInterval`,
`pollInterval`, `getTrackMetadata`, `getCrates`, `getCrate`, `findCratesForTrack`,
`getLibraryTracks({limit,offset})`, `getTrackByPath`, `searchLibrary`, and
`getDatabaseStats` are represented by same snake_case methods; pagination uses
`LibraryQuery`. Rust returns owned snapshots rather than exposing mutable
internal arrays. `getNowPlaying` retains its deprecated heuristic role; the
library does not claim that it determines on-air state.

## Types

The following original types retain their names in Rust (camelCase JSON fields):

- Logger; SeratoVersion; SeratoConnectOptions; SeratoConnectEvents;
  SeratoHistorySong; SeratoSession; SeratoReadyInfo; SeratoTrackPayload;
  SeratoHistoryPayload; SeratoSessionPayload; SeratoDeckChangePayload.
- SeratoColor; SeratoCuePoint; SeratoLoop; SeratoFlip; SeratoFlipAction;
  SeratoBeatgridMarker; SeratoBeatgrid; SeratoAutotags; SeratoMarkers2;
  SeratoTrackMetadata; SeratoCrate; SeratoDatabaseTrack.
- SeratoRemoteOptions; SeratoRemoteEvents; SeratoRemoteTrack;
  SeratoRemotePlayhead; SeratoRemoteLoopState; SeratoRemoteMixerState;
  SeratoRemoteDeckChangePayload; SeratoRemotePlayheadPayload;
  SeratoRemoteLoopPayload; SeratoRemoteMixerPayload; SeratoRemoteReadyInfo;
  SeratoRemotePeerInfo; MdnsPublication; MdnsPublishOptions; OscArg; OscMessage;
  RemoteSessionPeer; RemoteServerOptions; RemoteSessionOptions.
- Remote-only index concepts: RemoteServerEvents, RemoteSessionEvents,
  SeratoRemoteEmitter. Their Rust equivalents are typed event enums/receivers,
  rather than dynamic callback emitter interfaces.

Date values use epoch milliseconds in Rust and serialize to ISO8601 UTC
millisecond strings like the original JS Date objects. Optional fields are
omitted rather than fabricated. Byte buffers become Vec<u8>; metadata overview
normalizes to a JSON byte array. Explicit discovery contexts are injectable.

Rust additionally exposes bounded raw-data crate/database parsers, typed stats,
pure Remote state reduction, OSC bundle decoding and mDNS record construction
for fixture testing without native network publication.

## Native hostname resolver extension

The original listener `host` option accepts hostnames, so the Rust port preserves
that capability rather than restricting it to IP literals/localhost.
`RemoteServer::with_resolver` allows an injected `HostResolver` whose
`resolve(host, port, deadline)` returns owned socket addresses. Additional native
APIs are `BoundedResolver::with_lookup(HostLookup)`,
`BoundedResolver::shutdown(deadline)`, `NativeHostResolver`,
`numeric_host_address`, `RESOLVER_QUEUE_CAPACITY` (8), and
`MAX_RESOLVED_ADDRESSES` (32).

Numeric IPv4/IPv6 and legacy numeric IPv4 forms bypass DNS. Native hostname
lookups share one lazy worker across server instances/retries; expired queued
requests and late results cannot start a listener. The OS lookup itself may
remain blocked after the caller's deadline, as explained in README. Resolver
constructors and pure numeric parsing do not resolve a host, bind a socket or
publish mDNS.

## Deliberate differences

See README for bounded parser/IO behavior, failure categories, stronger ownership,
and the two original audio container bugs demonstrated by actual TS fixtures.
A Rust method's presence is source/API coverage, not proof of live software,
multicast discovery, packaged runtime or complete application integration.
