use lockgate_policy::ScopeRef;
use lockgate_schema::ScopeRef as HostScopeRef;

#[test]
fn authoring_scope_references_match_the_frozen_schema_wire() {
    for (author, host) in [
        (
            ScopeRef::literal("pool:gpu"),
            HostScopeRef::literal("pool:gpu").unwrap(),
        ),
        (
            ScopeRef::setting("/endpoint/~0name/~1path"),
            HostScopeRef::setting("/endpoint/~0name/~1path").unwrap(),
        ),
        (
            ScopeRef::root("workspace"),
            HostScopeRef::root("workspace").unwrap(),
        ),
        (
            ScopeRef::root("workspace").join("generated/html"),
            HostScopeRef::root("workspace")
                .unwrap()
                .join("generated/html")
                .unwrap(),
        ),
    ] {
        let wire = author_wire(&author);
        assert_eq!(wire, host.to_wire());
        assert_eq!(HostScopeRef::from_wire(&wire).unwrap(), host);
    }
}

#[test]
fn authoring_and_schema_scope_validation_have_matching_bounds() {
    let literal_limit = leak("l".repeat(2048));
    let literal_over_limit = leak("l".repeat(2049));
    for value in ["value", literal_limit] {
        assert_literal_parity(value, true);
    }
    for value in ["", "setting:/value", "$workspace", literal_over_limit] {
        assert_literal_parity(value, false);
    }

    let pointer_limit = leak(format!("/{}", "p".repeat(2047)));
    let pointer_over_limit = leak(format!("/{}", "p".repeat(2048)));
    for value in ["", "/endpoint/~0name/~1path", pointer_limit] {
        assert_setting_parity(value, true);
    }
    for value in ["endpoint", "/bad~", "/bad~2escape", pointer_over_limit] {
        assert_setting_parity(value, false);
    }

    let root_limit = leak("r".repeat(64));
    let root_over_limit = leak("r".repeat(65));
    for value in ["workspace", "r2-d2", root_limit] {
        assert_root_parity(value, true);
    }
    for value in ["", "Workspace", "two_words", "9root", root_over_limit] {
        assert_root_parity(value, false);
    }

    let subpath_limit = leak("s".repeat(1024));
    let subpath_over_limit = leak("s".repeat(1025));
    let depth_limit = leak(core::iter::repeat_n("s", 64).collect::<Vec<_>>().join("/"));
    let depth_over_limit = leak(core::iter::repeat_n("s", 65).collect::<Vec<_>>().join("/"));
    for value in ["generated/html", subpath_limit, depth_limit] {
        assert_join_parity(value, true);
    }
    for value in [
        "",
        "/absolute",
        ".",
        "..",
        "generated//html",
        "generated/",
        "generated/../html",
        "generated\\html",
        subpath_over_limit,
        depth_over_limit,
    ] {
        assert_join_parity(value, false);
    }
}

#[test]
fn authoring_and_schema_reject_the_same_unicode_character_classes() {
    const DISALLOWED_RANGES: &[(u32, u32)] = &[
        (0x0000, 0x001f),
        (0x007f, 0x009f),
        (0x00ad, 0x00ad),
        (0x0600, 0x0605),
        (0x061c, 0x061c),
        (0x06dd, 0x06dd),
        (0x070f, 0x070f),
        (0x0890, 0x0891),
        (0x08e2, 0x08e2),
        (0x180e, 0x180e),
        (0x200b, 0x200f),
        (0x2028, 0x202e),
        (0x2060, 0x2064),
        (0x2066, 0x206f),
        (0xfeff, 0xfeff),
        (0xfff9, 0xfffb),
        (0x110bd, 0x110bd),
        (0x110cd, 0x110cd),
        (0x13430, 0x1343f),
        (0x1bca0, 0x1bca3),
        (0x1d173, 0x1d17a),
        (0xe0001, 0xe0001),
        (0xe0020, 0xe007f),
    ];

    for &(start, end) in DISALLOWED_RANGES {
        for codepoint in start..=end {
            if let Some(character) = char::from_u32(codepoint) {
                let value = leak(character.to_string());
                assert_literal_parity(value, false);
                assert_setting_parity(leak(format!("/{value}")), false);
                assert_join_parity(value, false);
            }
        }
        for neighbor in [start.checked_sub(1), end.checked_add(1)]
            .into_iter()
            .flatten()
            .filter_map(char::from_u32)
        {
            if DISALLOWED_RANGES.iter().any(|&(other_start, other_end)| {
                (other_start..=other_end).contains(&(neighbor as u32))
            }) {
                continue;
            }
            let value = leak(neighbor.to_string());
            assert_literal_parity(value, true);
        }
    }
}

fn author_wire(reference: &ScopeRef) -> String {
    use lockgate_policy::__private::{scope_ref_wire_byte, scope_ref_wire_len};

    let bytes = (0..scope_ref_wire_len(reference))
        .map(|index| scope_ref_wire_byte(reference, index))
        .collect::<Vec<_>>();
    String::from_utf8(bytes).unwrap()
}

fn assert_literal_parity(value: &'static str, expected: bool) {
    assert_eq!(
        std::panic::catch_unwind(|| ScopeRef::literal(value)).is_ok(),
        expected,
        "author literal validation disagreed for {value:?}"
    );
    assert_eq!(
        HostScopeRef::literal(value).is_ok(),
        expected,
        "schema literal validation disagreed for {value:?}"
    );
}

fn assert_setting_parity(value: &'static str, expected: bool) {
    assert_eq!(
        std::panic::catch_unwind(|| ScopeRef::setting(value)).is_ok(),
        expected,
        "author setting validation disagreed for {value:?}"
    );
    assert_eq!(
        HostScopeRef::setting(value).is_ok(),
        expected,
        "schema setting validation disagreed for {value:?}"
    );
}

fn assert_root_parity(value: &'static str, expected: bool) {
    assert_eq!(
        std::panic::catch_unwind(|| ScopeRef::root(value)).is_ok(),
        expected,
        "author root validation disagreed for {value:?}"
    );
    assert_eq!(
        HostScopeRef::root(value).is_ok(),
        expected,
        "schema root validation disagreed for {value:?}"
    );
}

fn assert_join_parity(value: &'static str, expected: bool) {
    assert_eq!(
        std::panic::catch_unwind(|| ScopeRef::root("workspace").join(value)).is_ok(),
        expected,
        "author root join validation disagreed for {value:?}"
    );
    assert_eq!(
        HostScopeRef::root("workspace").unwrap().join(value).is_ok(),
        expected,
        "schema root join validation disagreed for {value:?}"
    );
}

fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}
