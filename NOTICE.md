# Source and license notices

The pinned original serato-connect TypeScript source is MIT, copyright2026
Chris Le. Its unchanged headers and source hashes are retained under
`tools/reference` and `fixtures/PROVENANCE.json`; the original MIT grant is
preserved in `LICENSE`.

The original encoding/GEOB source explicitly credits Jan Holthuis's
triseratops/serato-tags formats and algorithms. The seven-bit Serato32 transform
matches triseratops' MPL-2.0 implementation. To retain the applicable file-level
license rather than claim blanket MIT, this port distributes
`src/encoding/mod.rs` and `src/parsers/geob.rs` under MPL-2.0, with Jan Holthuis's
copyright and the complete license in `LICENSES/MPL-2.0.txt`. Other new Rust
files are MIT. The package SPDX expression is therefore `MIT AND MPL-2.0`.
Source for these covered files is included in this repository.

Upstream references checked during this port:

- triseratops Serato32 source, copyright2025 Jan Holthuis, MPL-2.0:
  https://github.com/Holzhaus/triseratops/blob/main/src/tag/serato32.rs
- serato-tags, copyright2019 Jan Holthuis, MIT:
  https://github.com/Holzhaus/serato-tags/blob/master/LICENSE
- serato-tools, copyright2025 bvandrc, MIT; its retained notices also credit
  Jan Holthuis and Brandon C.:
  https://github.com/bvandercar-vt/serato-tools/blob/master/LICENSE

Complete MIT reference notices are included under `LICENSES`; fetched license
URLs and checksums are recorded in fixture provenance. Existing protocol
constants are from the public original library, not user credentials. Generated
fixtures are synthetic and contain no personal audio, database, profile,
license token, or captured network traffic.

SQLite3MC and dependency licenses remain in their pinned upstream packages.
