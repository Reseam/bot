use super::*;

fn storage() -> Storage {
    Storage::new(StorageConfig {
        endpoint: "https://sf-objectstorage.com".to_owned(),
        region: "ca".to_owned(),
        bucket: "bucket-722-1280".to_owned(),
        prefix: "reseam-bot/shares/".to_owned(),
        access_key_id: "AKIDEXAMPLE".to_owned(),
        secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_owned(),
    })
    .expect("the test storage client builds")
}

fn at(time: &str) -> SystemTime {
    humantime::parse_rfc3339(time).expect("the test timestamp is valid")
}

#[test]
fn presigned_urls_match_a_reference_signer() {
    let storage = storage();
    let object = storage.presign(
        &Method::PUT,
        "reseam-bot/shares/706/1791469194000/x sharelinks+1.apk",
        &[],
        Duration::from_secs(900),
        at("2026-10-08T14:30:00Z"),
    );
    assert_eq!(
        object,
        "https://sf-objectstorage.com/bucket-722-1280/reseam-bot/shares/706/1791469194000/x%20sharelinks%2B1.apk?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIDEXAMPLE%2F20261008%2Fca%2Fs3%2Faws4_request&X-Amz-Date=20261008T143000Z&X-Amz-Expires=900&X-Amz-SignedHeaders=host&X-Amz-Signature=17b87adc3b2d27f226731b29775394f82d6f6ec4d349990a9640d3bad6f00982"
    );
    let listing = storage.presign(
        &Method::GET,
        "",
        &[
            ("list-type", "2"),
            ("prefix", "reseam-bot/shares/"),
            ("continuation-token", "a/b=c"),
        ],
        Duration::from_secs(900),
        at("2026-10-08T14:30:00Z"),
    );
    assert_eq!(
        listing,
        "https://sf-objectstorage.com/bucket-722-1280?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIDEXAMPLE%2F20261008%2Fca%2Fs3%2Faws4_request&X-Amz-Date=20261008T143000Z&X-Amz-Expires=900&X-Amz-SignedHeaders=host&continuation-token=a%2Fb%3Dc&list-type=2&prefix=reseam-bot%2Fshares%2F&X-Amz-Signature=cf2cb69b5da2f983697b749978560f3dc7215a62fd5cdd454dbb896f75f4ca1d"
    );
}
