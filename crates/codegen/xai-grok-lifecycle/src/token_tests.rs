use super::*;

#[test]
fn handler_name_follows_the_contract_pattern() {
    let longest = format!("a{}", "-".repeat(47));
    for accepted in [
        "a",
        "0",
        "myapp-flush",
        "chrome-cookies",
        "9lives",
        longest.as_str(),
    ] {
        assert!(HandlerName::try_from(accepted).is_ok(), "{accepted:?}");
    }
    let too_long = "a".repeat(49);
    for rejected in [
        "",
        "-lead",
        "Upper",
        "under_score",
        "dot.json",
        "../escape",
        "a/b",
        "sp ace",
        "é",
        too_long.as_str(),
    ] {
        assert!(
            matches!(
                HandlerName::try_from(rejected),
                Err(LifecycleError::InvalidHandlerName)
            ),
            "{rejected:?}"
        );
    }
}

#[test]
fn reason_follows_the_contract_pattern() {
    let longest = format!("a{}", "_".repeat(31));
    for accepted in [
        "manual",
        "idle_grace",
        "pre_ttl",
        "admit_lru",
        "reset",
        "a1",
        longest.as_str(),
    ] {
        assert!(ReasonToken::try_from(accepted).is_ok(), "{accepted:?}");
    }
    let too_long = "a".repeat(33);
    for rejected in [
        "",
        "1manual",
        "_x",
        "Idle",
        "idle-grace",
        "idle grace",
        too_long.as_str(),
    ] {
        assert!(
            matches!(
                ReasonToken::try_from(rejected),
                Err(LifecycleError::InvalidReason)
            ),
            "{rejected:?}"
        );
    }
}
