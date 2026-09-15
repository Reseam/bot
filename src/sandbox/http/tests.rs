use super::*;

#[test]
fn only_public_addresses_are_reachable() {
    let public = ["1.1.1.1", "38.45.64.161", "2606:4700:4700::1111"];
    let private = [
        "127.0.0.1",
        "10.0.1.5",
        "172.17.0.2",
        "192.168.1.112",
        "169.254.169.254",
        "100.64.0.1",
        "0.0.0.0",
        "::1",
        "fd00::1",
        "fe80::1",
        "::ffff:10.0.0.1",
        "64:ff9b::a00:1",
    ];
    for address in public {
        assert!(
            is_public(address.parse().expect("valid address")),
            "{address}"
        );
    }
    for address in private {
        assert!(
            !is_public(address.parse().expect("valid address")),
            "{address}"
        );
    }
}

#[test]
fn approval_preview_keeps_text_and_counts_binary_bytes() {
    let mut body = b"--b\r\nContent-Disposition: form-data; name=\"attachment\"; filename=\"clip.mp4\"\r\n\r\n".to_vec();
    body.extend_from_slice(&[0x00, 0x9f, 0xff, 0x10]);

    let preview = body_preview(&body);

    assert!(preview.starts_with("--b\r\nContent-Disposition"));
    assert!(preview.contains("filename=\"clip.mp4\""));
    assert!(preview.ends_with("[3 more bytes of binary data]"));
    assert_eq!(body_preview(b"{\"title\":\"ok\"}"), "{\"title\":\"ok\"}");
}
