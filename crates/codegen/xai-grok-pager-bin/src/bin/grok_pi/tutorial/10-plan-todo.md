# Plan Mode & Todo

Press `Ctrl+Shift+T` to toggle grok-pi Plan mode for the current Pi session.

- Pi can read and search the real repository before proposing an approach.
- The built-in grok-pi plan extension blocks normal `edit`, `write` and `bash`
  mutations except for the session-private plan file.
- The mode state is stored beside the Pi session and survives resume.
- Pi's `exit_plan_mode` bridge opens the native approval view so you can accept
  the plan or request changes before implementation.

Todo is a separate structured projection. grok-pi injects a built-in `todo`
tool by default; F2 `[ui].pi_todo` controls it and requires a restart. Its
`details.tasks` snapshots map to the native TodoPane, badge and ACP Plan instead
of rendering a duplicate tool card.

When built-in Todo is enabled, grok-pi's resource policy blocks the compatible
`npm:@juicesharp/rpiv-todo` provider so only one `todo` tool is registered. Turn
`pi_todo` off and restart before using that community provider instead. Plan
mode and Todo remain complementary: Plan gates mutations and approval, while
Todo tracks the live work list.
