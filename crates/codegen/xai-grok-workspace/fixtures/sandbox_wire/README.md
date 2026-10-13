# Sandbox card wire fixtures

The sandbox card on the wire, read by the Rust suite (`src/permission/sandbox_wire_tests.rs`)
and the desktop's card renderer tests. The chat relay keeps a byte-identical copy of
`card_fs_write_grantable.json`, pinned to this one by its
`sandbox_wire_fixture_matches_the_daemons_copy` test.

- `card_*.json` — the payload `grok-workspaced` posts on the permission channel for one
  violation: the pre-run keys (`tool_call_id`, `tool_name`, `description`, `scope`,
  `tool_approval_policy`) plus the card itself, repeated verbatim under `sandbox_violation`
  (the relay forwards that object). Written by the payload builder, never by hand:
  `SANDBOX_WIRE_FIXTURES_UPDATE=1 cargo test -p xai-grok-workspace --lib -- card_payloads_match_the_shared_fixtures`
  regenerates them after a wire change.
- `reply_*.json` — a desktop answer as the daemon receives it. Hand-written; the Rust test
  `reply_fixtures_decode_as_documented` pins what each decodes to.
