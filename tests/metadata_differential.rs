use serato_connect_rs::*;
use serde_json::{json, Value};
use std::{fs, path::PathBuf};
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/metadata")
}
fn bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|v| u8::from_str_radix(std::str::from_utf8(v).unwrap(), 16).unwrap())
        .collect()
}
fn hex(data: impl AsRef<[u8]>) -> String {
    data.as_ref().iter().map(|v| format!("{v:02x}")).collect()
}
fn value<T: serde::Serialize>(result: Result<T>) -> Value {
    match result {
        Ok(value) => json!({"ok":value}),
        Err(_) => json!({"error":true}),
    }
}
fn normalize(v: &mut Value) {
    match v {
        Value::Number(n) => *v = json!(n.as_f64().unwrap()),
        Value::Array(a) => a.iter_mut().for_each(normalize),
        Value::Object(o) => o.values_mut().for_each(normalize),
        _ => {}
    }
}
#[test]
fn all_metadata_parser_codec_and_audio_cases_match_actual_pinned_typescript() {
    let fixture: Vec<Value> =
        serde_json::from_str(include_str!("../fixtures/metadata-expected.json")).unwrap();
    assert!(fixture.len() >= 100);
    for (index, c) in fixture.iter().enumerate() {
        let op = c["op"].as_str().unwrap();
        let folder = c["folder"]
            .as_str()
            .map(|p| root().join(p))
            .unwrap_or_else(root);
        let data = c["hex"].as_str().map(bytes).unwrap_or_default();
        let s = c["text"].as_str().unwrap_or_default();
        let mut actual = match op {
            "encodeSerato32" => {
                let a = c["values"].as_array().unwrap();
                value(Ok(encode_serato32(
                    a[0].as_u64().unwrap() as u8,
                    a[1].as_u64().unwrap() as u8,
                    a[2].as_u64().unwrap() as u8,
                )))
            }
            "decodeSerato32" => {
                let a = c["values"].as_array().unwrap();
                value(Ok(decode_serato32(
                    a[0].as_u64().unwrap() as u8,
                    a[1].as_u64().unwrap() as u8,
                    a[2].as_u64().unwrap() as u8,
                    a[3].as_u64().unwrap() as u8,
                )))
            }
            "encodeSerato32Buffer" => value(encode_serato32_buffer(&data).map(hex)),
            "decodeSerato32Buffer" => value(decode_serato32_buffer(&data).map(hex)),
            "encodeSerato32U32" => value(Ok(hex(encode_serato32_u32(
                c["value"].as_u64().unwrap() as u32,
            )))),
            "decodeSerato32U32" => value(decode_serato32_u32(
                &data,
                c["offset"].as_u64().unwrap() as usize,
            )),
            "encodeSerato32Color" => value(Ok(hex(encode_serato32_color(
                &serde_json::from_value(c["color"].clone()).unwrap(),
            )))),
            "decodeSerato32Color" => value(decode_serato32_color(
                &data,
                c["offset"].as_u64().unwrap() as usize,
            )),
            "encodeWithLinebreaks" => value(encode_with_linebreaks(&data)),
            "decodeWithLinebreaks" => value(decode_with_linebreaks(s).map(hex)),
            "isSeratoBase64" => value(Ok(is_serato_base64(s))),
            "parseMarkers2" => value(parse_markers2(&data)),
            "parseMarkers2FromBase64" => value(parse_markers2_from_base64(s)),
            "parseMarkers2FromGeob" => value(parse_markers2_from_geob(&data)),
            "parseBeatgrid" => value(parse_beatgrid(&data)),
            "parseBeatgridFromBase64" => value(parse_beatgrid_from_base64(s)),
            "getBpmFromBeatgrid" => value(Ok(get_bpm_from_beatgrid(
                &serde_json::from_value(c["grid"].clone()).unwrap(),
            ))),
            "isDynamicBeatgrid" => value(Ok(is_dynamic_beatgrid(
                &serde_json::from_value(c["grid"].clone()).unwrap(),
            ))),
            "parseAutotags" => value(parse_autotags(&data)),
            "parseAutotagsFromBase64" => value(parse_autotags_from_base64(s)),
            "parseCrate" | "parseCrateSync" => value(
                parse_crate(root().join(c["file"].as_str().unwrap())).map(|mut v| {
                    v.path = c["file"].as_str().unwrap().into();
                    v
                }),
            ),
            "listCrates" => value(list_crates(&folder).map(|paths| {
                paths
                    .into_iter()
                    .map(|p| {
                        std::path::Path::new(&p)
                            .strip_prefix(root())
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/")
                    })
                    .collect::<Vec<_>>()
            })),
            "getAllCrates" => value(get_all_crates(root()).map(|mut crates| {
                for v in &mut crates {
                    v.path = std::path::Path::new(&v.path)
                        .strip_prefix(root())
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                }
                crates
            })),
            "findCratesForTrack" => {
                value(find_crates_for_track(root(), c["track"].as_str().unwrap()))
            }
            "parseDatabase" | "parseDatabaseSync" => {
                value(parse_database(root().join(c["file"].as_str().unwrap())))
            }
            "getLibraryTracks" => value(get_library_tracks(root())),
            "getTrackByPath" => value(get_track_by_path(root(), c["track"].as_str().unwrap())),
            "searchLibrary" => value(search_library(root(), c["query"].as_str().unwrap())),
            "getDatabaseStats" => value(get_database_stats(&folder)),
            "getTrackMetadata" => {
                value(get_track_metadata(root().join(c["file"].as_str().unwrap())))
            }
            "getTrackMetadataFromBuffer" => value(get_track_metadata_from_buffer(
                &fs::read(root().join(c["file"].as_str().unwrap())).unwrap(),
                c["mime"].as_str().unwrap(),
            )),
            _ => panic!("unhandled original export {op}"),
        };
        let mut expected = if c.get("correction").is_some() {
            assert_eq!(c["expected"], json!({"ok":null}));
            c["expectedNative"].clone()
        } else {
            c["expected"].clone()
        };
        normalize(&mut actual);
        normalize(&mut expected);
        assert_eq!(actual, expected, "case {index}: {op}");
    }
}
#[test]
fn fixture_hashes_are_preserved_by_file_readers() {
    use sha2::{Digest, Sha256};
    let hashes: std::collections::BTreeMap<String, String> =
        serde_json::from_str(include_str!("../fixtures/metadata-hashes.json")).unwrap();
    for (name, expected) in hashes {
        let p = root().join(name);
        let before = fs::read(&p).unwrap();
        assert_eq!(hex(Sha256::digest(&before)), expected);
        let _ = get_track_metadata(&p);
        assert_eq!(fs::read(&p).unwrap(), before);
    }
}
