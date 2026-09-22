use std::io::{Cursor, Write};

use base64::Engine;
use zip::{ZipWriter, write::SimpleFileOptions};

use super::{MAX_BYTES, MAX_ENTRIES, zip as format};

#[test]
fn recursive_zip_preserves_binary_empty_directories_and_unicode() {
    let entries = vec![
        ("project".into(), None),
        ("project/empty".into(), None),
        (
            "project/src/binary.dat".into(),
            Some(vec![0, 255, 128, 13, 10]),
        ),
        (
            "project/résumé.txt".into(),
            Some("Hello 世界".as_bytes().to_vec()),
        ),
        ("project/zero.txt".into(), Some(vec![])),
    ];
    let bytes = format::compress(entries.clone()).unwrap();
    let (listing, _) = format::read(bytes.clone(), false).unwrap();
    assert!(listing.contains("dir\t0\tproject/empty"));
    let (_, extracted) = format::read(bytes, true).unwrap();
    for ((name, expected), actual) in entries.into_iter().zip(extracted) {
        assert_eq!(actual.name, name);
        assert_eq!(
            actual
                .base64
                .map(|data| base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap()),
            expected
        );
    }
}

#[test]
fn unsafe_paths_duplicates_and_file_directory_conflicts_are_rejected() {
    for name in [
        "",
        "/absolute",
        "../escape",
        "a/../../escape",
        "a/./b",
        "a//b",
        "a\\b",
        "C:escape",
        "a\0b",
    ] {
        assert!(
            format::validate_entries([(name, false)]).is_err(),
            "{name:?}"
        );
    }
    assert!(format::validate_entries([("a", false), ("a", false)]).is_err());
    assert!(format::validate_entries([("a", false), ("a/b", false)]).is_err());
    assert!(format::validate_entries([("a/b", false), ("a", false)]).is_err());
    assert!(format::validate_entries([("a/b", false), ("a", true)]).is_ok());
    let deep = vec!["a"; 65].join("/");
    assert!(format::validate_entries([(deep.as_str(), false)]).is_err());
    let names = (0..=MAX_ENTRIES).map(|i| i.to_string()).collect::<Vec<_>>();
    assert!(format::validate_entries(names.iter().map(|name| (name.as_str(), false))).is_err());
}

#[test]
fn malicious_zip_paths_and_symlinks_are_rejected() {
    for name in ["../escape", "/escape", "C:escape", "a\\b"] {
        let bytes = format::compress(vec![(name.into(), Some(vec![1]))]).unwrap();
        assert!(format::read(bytes, true).is_err());
    }
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    archive
        .add_symlink("link", "../outside", SimpleFileOptions::default())
        .unwrap();
    assert!(format::read(archive.finish().unwrap().into_inner(), true).is_err());
}

#[test]
fn expanded_size_limits_are_enforced_before_extraction() {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    archive
        .start_file("large", SimpleFileOptions::default())
        .unwrap();
    let chunk = [0; 1024 * 1024];
    for _ in 0..=MAX_BYTES / chunk.len() {
        archive.write_all(&chunk).unwrap();
    }
    let bytes = archive.finish().unwrap().into_inner();
    assert!(format::read(bytes.clone(), false).is_err());
    assert!(format::read(bytes, true).is_err());
}

#[test]
fn corrupt_zip_is_rejected() {
    assert!(format::read(b"not a zip".to_vec(), true).is_err());
    let mut bytes = format::compress(vec![("file".into(), Some(vec![0, 1, 2]))]).unwrap();
    bytes.truncate(bytes.len() / 2);
    assert!(format::read(bytes, true).is_err());
}
