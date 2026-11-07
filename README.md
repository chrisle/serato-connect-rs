# serato-connect-rs

Native Rust port of the public `chrisle/serato-connect`1.4.6 source at
`8ce4aaf7642968fbde21ad413eff5b89db4debf4`: V3/V4 history, deck state,
crate/library queries, Serato32/Base64, GEOB tags, audio containers, OSC Remote
sessions/server/client and mDNS publication. [EXPORTS.md](EXPORTS.md) maps the
complete original export surface and connector forwarding methods.

This is a local review candidate. It has not been published, integrated into
Now Playing, or verified with a live Serato installation or physical hardware.

```rust,no_run
use serato_connect_rs::{SeratoConnect, SeratoConnectOptions, SeratoEvent};
let mut connector = SeratoConnect::new(SeratoConnectOptions {
    serato_path: Some("/explicit/synthetic/_Serato_".into()),
    ..Default::default()
});
connector.start()?;
// Continuously consume connector.events(); initial Ready/DeckChange state is
// separate from subsequent History plays. Serato does not determine on-air.
connector.stop();
# Ok::<(),serato_connect_rs::Error>(())
```

`SeratoConnect::new`, `SeratoRemoteClient::new`, `RemoteServer::new` and options
construction are passive. History starts only on start; TCP and mDNS start only
on explicit server/client start or publish calls. `RemoteSession::new` is the
explicit handoff of an already accepted TCP socket. Paths and discovery contexts
can be injected; unattended tests never read a personal library/profile or
advertise on the LAN. Calling default-discovery helpers explicitly resolves
real platform locations, as their names imply.

## One SQLite engine

V4 `master.sqlite` is plaintext and opens read-only. It uses the same Cargo-built
rusqlite0.37/libsqlite3-sys0.35 SQLite3MC2.3.5/SQLite3.53.2 engine used by the
other Rust connector ports. No system SQLite, shell prebuild or second link
owner is introduced. Consumers must repeat the patch at their workspace root:

```toml
[dependencies]
serato-connect-rs = { path = "vendor/serato-connect-rs" }
[patch.crates-io]
libsqlite3-sys = { git = "https://github.com/chrisle/onelibrary-connect-rs", rev = "b207f59fc87983286b679b4d9072fd9caa81e89a" }
```

Dependency-local patches are not inherited. Preserve that dependency's source
hash checks and license notices. This candidate does not modify the original
TypeScript package, application workspace or any Serato database.

## History and Remote lifecycle

V3 startup seeds old history and emits initial loaded-deck state separately.
Later batches retain source ordering and the original capped history payload
contract: count/cursor cover all newly observed rows while tracks contains the
trailing historyMaxRows. The played flag participates in deck identity. Native
polls are serialized; V4 automatically detects the actual active library, honors
an explicitly supplied library path and rereads in-place played/end-time
changes even when row count is unchanged. These fix known original lifecycle
bugs. Session shrink clamps the history cursor instead of permanently ignoring
later rows. Explicit stop clears the run and old events.

Remote is an inbound TCP listener plus an explicitly requested mDNS advert.
The server shares its UUID across parallel streams, computes the original
nonce challenge response, pairs/subscribes once, echoes Ping and handles
sentinel-framed messages/bundles. Native state preserves the original50ms
per-deck field-settle window, immediate eject and1-based deck slots. Raw mixer
values remain library data; application on-air/controller processing is outside
this crate.

TCP listener hosts accept configured hostnames as well as numeric IPv4/IPv6.
`numeric_host_address` also preserves legacy IPv4 shorthand, octal, hexadecimal
and integer forms such as `127.1`; numeric addresses bypass DNS entirely.
`RemoteServer::with_resolver` accepts an injected `HostResolver` for embedding
and controlled tests. The default `NativeHostResolver` shares one lazy lookup
worker across all servers and retries; construction starts no lookup/thread.
`BoundedResolver::with_lookup` provides the same behavior for an injected
lookup, with an eight-request queue and at most 32 resolved addresses.

Listener startup shares a deadline of one second across resolution and bind attempts.
Expired/cancelled queued lookups are skipped and late answers discarded. OS
`getaddrinfo` cannot be interrupted: a stalled native call occupies that one
worker, while callers can time out without spawning replacement threads.
`BoundedResolver::shutdown(deadline)` joins a released worker or reports an
unconfirmed cleanup deadline; it cannot promise to stop a kernel-stalled lookup.
Resolver tests inject synthetic answers/errors and controlled blocked calls;
no external DNS lookup or LAN advertisement is used as test evidence.

History events are bounded256. Remote standalone events256/outbound16,
server aggregate512 and client events512; saturation retires ownership instead
of growing without bound. Peer default32/hard128; frame default1MiB/hard16MiB;
bundle depth32/messages4096. Handshake deadline30s, idle60s, read polling20ms,
write timeout250ms. Shutdown signals all peers before joining. Native mDNS
unregister and daemon shutdown each have a1s wait; an error does not falsely
claim successful external discovery or cleanup. Tests inject the publisher and
use only loopback sockets, never an actual multicast advert.

## Metadata and audio

All encoding, parser and query APIs are native. Audio readers extract only
Serato metadata, not generic title/duration or cover images:

- ID3v2.2/2.3/2.4 GEOB in MP3, AIFF/AIFC and WAV RIFF chunks, including UTF16 descriptions.
- FLAC and Ogg Vorbis comments, case-insensitive Serato field names.
- Raw `com.serato.dj` freeform MP4/M4A atoms.
- Markers2 cues/loops/flips/color/BPM lock, Beatgrid, Autotags and raw Overview.

File readers seek past audio payload rather than loading/decoding an entire
track. Buffer readers inspect supplied bytes only. Individual invalid Serato
fields do not discard other valid fields; no useful metadata returns None.
Async JS IO methods map to synchronous Rust methods suitable for the caller's
blocking executor; asynchronous behavior is not simulated with a Node wrapper.

The actual original TypeScript reader and music-metadata11.12.3 demonstrate two
bugs in generated audio fixtures: ID3v2.2 emits native `GEO` rather than `GEOB`,
and unknown raw MP4 atoms are discarded by music-metadata. The original reader
therefore returns null for these advertised formats. Native Rust explicitly
recognizes both and returns their Serato tags. Fixture records retain the
original null and a separately labelled native expectation obtained from the
actual original GEOB parsers. These are intentional corrections, not identical
output claims.

JavaScript UTF16 lexical sorting is preserved for crate names and genres.
Malformed lone-surrogate text uses Unicode replacement instead of exposing
invalid Rust strings. Unbounded/error-prone binary reads are checked; short
Serato32 integer reads return an error instead of coercing undefined bytes to0.

## Bounds and error handling

Binary library files and supplied buffers are bounded at64MiB; individual audio
metadata blocks16MiB, accumulated metadata reads64MiB, parser records1million,
MP4 recursion16 levels and Ogg header pages10000. ResourceLimit is explicit;
parsers do not silently truncate a successful result at these safety bounds.
Audio file size itself is not64MiB-limited: a sparse256MiB payload test proves
metadata extraction seeks past it. Unknown binary entries remain compatible;
truncated crate/database tails retain successfully parsed earlier records,
matching the source.

Only regular file handles are accepted. Unix nonblocking opens plus pre/post
metadata checks prevent FIFO/device races. Filesystem IO stalled inside the
kernel cannot be interrupted by a user-space deadline; callers must not run
blocking readers on the UI thread. Errors use static categories and never embed
source file paths, private rows or underlying native errors.

History/Remote event queues and actors have separate explicit ownership and
bounds described above. No silence-based on-air inference or hardware identity
is manufactured.

## Validation

```sh
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
npm ci --prefix tools --no-audit --no-fund
node tools/regenerate.cjs
node tools/runtime/regenerate.cjs
```

The metadata oracle executes the actual checksum-pinned original TypeScript,
not a copied JS reimplementation. Generated fixtures cover every encoding/GEOB
function, crate/database V2 queries, Unicode sort order, metadata facade
forwarding, and file/buffer MP3, AIFF, WAV, FLAC, Ogg and M4A containers. Fixture
hash-preservation tests verify readers do not modify their sources. Prefix and
length regressions, FIFO rejection and sparse large-media seeking exercise
malformed/resource boundaries. Fixture provenance/hashes and original source
copies are included. Runtime fixtures/oracles cover their own history/OSC
contracts; none of these replaces multicast discovery or a real Serato session.

Native Windows/macOS/Linux CI, real Serato coexistence/discovery, physical
hardware, platform packaging and full Now Playing adapter/controller-state
integration remain independent gates until their evidence is recorded. No
crates.io publication, release tags, app deployment or original TS changes are
part of this local candidate.

## Licensing

This repository is **MIT AND MPL-2.0**, not blanket MIT. Original Serato source
MIT attribution is preserved. Local encoding/GEOB files retain the MPL2-derived
format algorithm attribution and applicable file-level notices. See
[NOTICE.md](NOTICE.md), `LICENSE`, `LICENSES/` and fixture provenance. No private
captures, personal media or user credentials are included.
