use pretty_assertions::assert_eq;

use super::*;

fn wire(argv: Vec<String>, timeout_ms: Option<u64>) -> HandlerSpecWire {
    HandlerSpecWire {
        v: 1,
        argv,
        timeout_ms,
    }
}

fn args(argv: &[&str]) -> Vec<String> {
    argv.iter().map(|arg| (*arg).to_owned()).collect()
}

#[cfg(unix)]
#[test]
fn registration_body_round_trips_through_the_contract_example() {
    let example = r#"{"v":1,"argv":["/usr/local/bin/myapp","flush"],"timeout_ms":5000}"#;
    let spec = ExecSpec::from_wire(serde_json::from_str(example).expect("parse")).expect("valid");
    assert_eq!(&args(&["/usr/local/bin/myapp", "flush"])[..], spec.argv());
    assert_eq!(Some(Duration::from_secs(5)), spec.timeout());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(example).expect("value"),
        serde_json::to_value(spec.to_wire()).expect("encode")
    );
}

#[cfg(unix)]
#[test]
fn argv_must_be_non_empty_absolute_and_bounded() {
    let at_cap = vec!["/".to_owned(), "x".repeat(MAX_ARGV_BYTES - 1)];
    assert!(ExecSpec::from_wire(wire(at_cap, None)).is_ok());

    let over_cap = vec!["/".to_owned(), "x".repeat(MAX_ARGV_BYTES)];
    for argv in [
        Vec::new(),
        args(&["relative/bin"]),
        args(&["bin"]),
        args(&["/bin/sh", "nul\0byte"]),
        over_cap,
    ] {
        assert_eq!(
            Err(ErrorClass::InvalidArgv),
            ExecSpec::from_wire(wire(argv.clone(), None)),
            "{argv:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn timeout_is_optional_and_range_checked() {
    let argv = args(&["/bin/true"]);
    assert_eq!(
        None,
        ExecSpec::from_wire(wire(argv.clone(), None))
            .expect("valid")
            .timeout()
    );
    for ms in [MIN_TIMEOUT_MS, MAX_TIMEOUT_MS] {
        assert!(ExecSpec::from_wire(wire(argv.clone(), Some(ms))).is_ok());
    }
    for ms in [0, MIN_TIMEOUT_MS - 1, MAX_TIMEOUT_MS + 1] {
        assert_eq!(
            Err(ErrorClass::InvalidTimeout),
            ExecSpec::from_wire(wire(argv.clone(), Some(ms)))
        );
    }
}

#[test]
fn other_versions_are_rejected() {
    let spec = HandlerSpecWire {
        v: 2,
        argv: args(&["/bin/true"]),
        timeout_ms: None,
    };
    assert_eq!(
        Err(ErrorClass::UnsupportedVersion),
        ExecSpec::from_wire(spec)
    );
}
