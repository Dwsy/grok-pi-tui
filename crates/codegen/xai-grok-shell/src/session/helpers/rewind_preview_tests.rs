use super::*;

#[test]
fn preview_is_the_first_non_empty_line_of_the_user_query() {
    assert_eq!(
        Some("fix the parser".to_owned()),
        rewind_prompt_preview("<user_query>\n\n  fix the parser  \nand add tests\n</user_query>")
    );
}

#[test]
fn long_first_line_is_cut_to_the_preview_width() {
    let preview = rewind_prompt_preview(&"a".repeat(80)).expect("a preview");

    assert_eq!(format!("{}...", "a".repeat(57)), preview);
}

#[test]
fn blank_prompt_has_no_preview() {
    assert_eq!(None, rewind_prompt_preview(" \n\t\n"));
}

#[test]
fn ms_to_rfc3339_handles_epoch_and_invalid() {
    assert!(ms_to_rfc3339(0).starts_with("1970-01-01T"));
    assert_eq!(ms_to_rfc3339(i64::MAX), "");
}
