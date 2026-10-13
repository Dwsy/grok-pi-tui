use super::*;

fn grant(expires: Expiry) -> Grant {
    Grant {
        id: GrantId::new("0192c1a0-0000-7000-8000-000000000001"),
        subject: GrantSubject::FsWriteRoot {
            root: PathBuf::from("/ws/node_modules"),
        },
        scope: GrantScope::Workspace {
            root: PathBuf::from("/ws"),
        },
        expires,
        decision: GrantDecision::Allow,
        granted_at: 1_758_400_000,
        granted_by: "cli".to_owned(),
        via: None,
    }
}

#[test]
fn ttl_grant_is_live_until_its_deadline_and_not_at_it() {
    let g = grant(Expiry::Ttl { seconds: 60 });
    assert_eq!(Some(1_758_400_060), g.expires_at());
    assert!(g.is_live(1_758_400_059));
    assert!(!g.is_live(1_758_400_060));
}

#[test]
fn never_grant_has_no_deadline() {
    let g = grant(Expiry::Never);
    assert_eq!(None, g.expires_at());
    assert!(g.is_live(i64::MAX));
}

#[test]
fn fixed_clock_advances_past_a_ttl() {
    let clock = FixedClock::at(1_758_400_000);
    let g = grant(Expiry::Ttl { seconds: 1 });
    assert!(g.is_live(clock.now_unix()));
    clock.advance(1);
    assert!(!g.is_live(clock.now_unix()));
}

#[test]
fn host_pattern_matches_exact_wildcard_and_all() {
    let exact = HostPattern::new("Registry.npmjs.org");
    assert!(exact.matches("registry.npmjs.org"));
    assert!(exact.matches("registry.npmjs.org."));
    assert!(!exact.matches("npmjs.org"));
    assert!(!exact.matches("evil-registry.npmjs.org"));

    let wildcard = HostPattern::new("*.example.com");
    assert!(wildcard.matches("api.example.com"));
    assert!(wildcard.matches("a.b.example.com"));
    assert!(
        !wildcard.matches("example.com"),
        "the suffix itself is not a subdomain"
    );
    assert!(!wildcard.matches("notexample.com"));
    assert!(!wildcard.matches("example.com.evil"));

    assert!(HostPattern::all().matches("anything"));
    assert!(!HostPattern::new("*.").matches("x"));
}

#[test]
fn host_pattern_coverage_is_the_deny_shadowing_rule() {
    let all = HostPattern::all();
    let wide = HostPattern::new("*.example.com");
    let narrow = HostPattern::new("*.a.example.com");
    let one = HostPattern::new("api.example.com");
    let other = HostPattern::new("api.other.com");

    assert!(one.is_covered_by(&all));
    assert!(wide.is_covered_by(&all));
    assert!(!all.is_covered_by(&wide));
    assert!(one.is_covered_by(&wide));
    assert!(narrow.is_covered_by(&wide));
    assert!(!wide.is_covered_by(&narrow));
    assert!(!other.is_covered_by(&wide));
    assert!(wide.is_covered_by(&wide));
    assert!(!HostPattern::new("example.com").is_covered_by(&wide));
}

/// An address is one host however it is spelled (brackets, IPv4-mapped, zero runs), and no
/// wildcard reaches it by its digits; a deny row for one spelling covers an allow for another.
#[test]
fn an_address_is_matched_by_value_and_never_by_a_wildcard() {
    let v4 = HostPattern::new("10.0.0.5");
    assert!(v4.matches("10.0.0.5"));
    assert!(v4.matches("::ffff:10.0.0.5"));
    assert!(v4.matches("[::ffff:a00:5]"));
    assert!(!v4.matches("10.0.0.50"));
    let v6 = HostPattern::new("[::1]");
    assert!(v6.matches("::1"));
    assert!(v6.matches("0:0:0:0:0:0:0:1"));
    assert!(!v6.matches("127.0.0.1"));
    for wildcard in ["*.0.0.1", "*.1", "*.0.5"] {
        let pattern = HostPattern::new(wildcard);
        assert!(!pattern.matches("127.0.0.1"), "{wildcard}");
        assert!(!pattern.matches("10.0.0.5"), "{wildcard}");
    }
    assert!(HostPattern::all().matches("[::1]"));
    assert!(HostPattern::new("::ffff:10.0.0.5").is_covered_by(&v4));
    assert!(!HostPattern::new("127.0.0.1").is_covered_by(&HostPattern::new("*.0.0.1")));
}

/// A bracketed IPv6 literal splits at the `:` after its closing bracket and loses the brackets;
/// an unbracketed one is never cut at its own colons; anything malformed is a host as written.
#[test]
fn host_port_splits_at_the_port_separator_outside_ipv6_brackets() {
    for (value, split) in [
        ("[::1]:443", ("::1", Some("443"))),
        ("[2001:db8::1]:8443", ("2001:db8::1", Some("8443"))),
        ("host:443", ("host", Some("443"))),
        ("host", ("host", None)),
        ("[::1]", ("::1", None)),
        ("2001:db8::1", ("2001:db8::1", None)),
        ("host:", ("host", Some(""))),
        ("[::1]443", ("[::1]443", None)),
        ("[::1:443", ("[::1:443", None)),
    ] {
        assert_eq!(split, split_host_port(value), "{value}");
    }
}

#[test]
fn wildcard_coverage_ignores_case_and_trailing_dots() {
    let deny = HostPattern::new("*.example.com");
    assert!(HostPattern::new("*.Example.COM").is_covered_by(&deny));
    assert!(HostPattern::new("*.example.com.").is_covered_by(&deny));
    assert!(deny.is_covered_by(&HostPattern::new("*.EXAMPLE.com.")));
    assert!(!HostPattern::new("*.Example.org").is_covered_by(&deny));
}

#[test]
fn an_apex_deny_never_covers_the_wildcard_allow_of_its_subdomains() {
    let wildcard = HostPattern::new("*.example.com");
    assert!(!wildcard.is_covered_by(&HostPattern::new("example.com")));
    assert!(!wildcard.is_covered_by(&HostPattern::new("Example.COM.")));
    assert!(!HostPattern::new("*.a.example.com").is_covered_by(&HostPattern::new("a.example.com")));
    assert!(HostPattern::new("*.a.example.com").is_covered_by(&wildcard));
}

#[test]
fn grant_row_round_trips_through_toml() {
    let g = grant(Expiry::At {
        unix: 1_758_500_000,
    });
    let text = toml::to_string(&g).expect("serialize");
    let back: Grant = toml::from_str(&text).expect("deserialize");
    assert_eq!(g, back);
    assert!(text.contains("kind = \"fs_write_root\""));
    assert!(text.contains("kind = \"workspace\""));
}

/// The family grant is the bare kind on both wires: `kind = "build_caches"`
/// in the store, `{ "kind": "build_caches" }` on the card — no path, the table is the daemon's.
#[test]
fn build_caches_subject_is_the_bare_kind_in_toml_and_json() {
    let mut g = grant(Expiry::Ttl {
        seconds: 7 * 86_400,
    });
    g.subject = GrantSubject::BuildCaches;
    let text = toml::to_string(&g).expect("serialize");
    assert!(
        text.contains("[subject]\nkind = \"build_caches\"\n"),
        "{text}"
    );
    let back: Grant = toml::from_str(&text).expect("deserialize");
    assert_eq!(g, back);
    let json = serde_json::to_value(&g.subject).unwrap();
    assert_eq!(serde_json::json!({ "kind": "build_caches" }), json);
    let back: GrantSubject = serde_json::from_value(json).unwrap();
    assert_eq!(GrantSubject::BuildCaches, back);
}
