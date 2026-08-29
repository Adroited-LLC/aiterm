//! The desktop side of Remote Access: what the settings panel calls.
//!
//! These cover the parts that decide trust or hand out a secret. The Tauri
//! command wrappers themselves are one-line adapters over the functions
//! exercised here.

use aiterm_lib::remote::{pairing_payload, pairing_qr_svg, shareable_addresses, PairingUri};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[test]
fn a_pairing_uri_carries_every_field_the_phone_needs_to_pin() {
    let payload = pairing_payload(
        &["192.168.1.20".parse().unwrap(), "10.8.0.3".parse().unwrap()],
        8443,
        "AbCdEf0123456789",
        b"\x00\x01\x02\x03",
        "Matt's desktop",
    );
    let parsed = PairingUri::parse(&payload).expect("the desktop must emit a payload it can read");

    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.hosts, vec!["192.168.1.20", "10.8.0.3"]);
    assert_eq!(parsed.port, 8443);
    assert_eq!(parsed.fingerprint, "AbCdEf0123456789");
    assert_eq!(parsed.secret, b"\x00\x01\x02\x03");
    assert_eq!(parsed.name, "Matt's desktop");
}

#[test]
fn a_desktop_name_with_punctuation_survives_the_round_trip() {
    // The name is shown to the user as the thing they are agreeing to pair
    // with. An ampersand or a space that mangles the URI would either break
    // the parse or, worse, silently truncate the name into something else.
    let payload = pairing_payload(
        &["192.168.1.20".parse().unwrap()],
        8443,
        "fp",
        b"secret",
        "Matt & Ada's box #2",
    );
    let parsed = PairingUri::parse(&payload).unwrap();
    assert_eq!(parsed.name, "Matt & Ada's box #2");
}

#[test]
fn an_ipv6_candidate_is_not_split_by_its_colons() {
    let payload = pairing_payload(
        &["fd00::1".parse().unwrap(), "192.168.1.20".parse().unwrap()],
        8443,
        "fp",
        b"secret",
        "desktop",
    );
    let parsed = PairingUri::parse(&payload).unwrap();
    assert_eq!(parsed.hosts, vec!["fd00::1", "192.168.1.20"]);
}

#[test]
fn a_payload_of_another_version_is_refused_rather_than_guessed_at() {
    let payload = pairing_payload(&["192.168.1.20".parse().unwrap()], 8443, "fp", b"s", "d");
    let future = payload.replace("v=1", "v=2");
    assert!(
        PairingUri::parse(&future).is_none(),
        "a version this build does not know governs trust: it must not be parsed"
    );
}

#[test]
fn a_payload_missing_a_trust_field_is_refused() {
    for missing in ["f=", "s=", "p="] {
        let payload = pairing_payload(&["192.168.1.20".parse().unwrap()], 8443, "fp", b"s", "d");
        let broken = payload
            .split('&')
            .filter(|part| !part.starts_with(missing))
            .collect::<Vec<_>>()
            .join("&");
        assert!(
            PairingUri::parse(&broken).is_none(),
            "a payload without {missing} cannot be trusted: {broken}"
        );
    }
}

#[test]
fn loopback_and_unusable_addresses_are_never_offered_as_bind_candidates() {
    let candidates = shareable_addresses(vec![
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
        IpAddr::V4(Ipv4Addr::new(169, 254, 3, 4)),
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
        IpAddr::V4(Ipv4Addr::new(10, 8, 0, 3)),
    ]);

    // A phone cannot reach loopback, and a link-local address is not a
    // network anyone routes a phone over. Offering either can only produce a
    // listener the user then has to debug.
    assert_eq!(
        candidates,
        vec!["192.168.1.20".to_string(), "10.8.0.3".to_string()]
    );
}

#[test]
fn the_qr_is_an_svg_that_encodes_the_payload_and_nothing_larger() {
    let payload = pairing_payload(
        &["192.168.1.20".parse().unwrap()],
        8443,
        "n1oZ8kQ2xr7Yv0bDq3sTfLmE5wUcJhAaP9gRkNzXeIo",
        &[7u8; 32],
        "Matt's desktop",
    );
    let svg = pairing_qr_svg(&payload).expect("a payload this size must render");

    assert!(svg.starts_with("<svg"), "the renderer must emit a bare SVG element: {}", &svg[..40.min(svg.len())]);
    assert!(!svg.contains("<?xml"), "an XML prolog cannot be injected into a DOM node");
    // The secret must survive as drawn geometry only — never as text a
    // screenshot tool, an accessibility tree, or a log could lift out.
    assert!(!svg.contains("aiterm://"), "the payload must not appear as text in the SVG");
}
