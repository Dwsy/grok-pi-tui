# Review, Timeline & Rollback

grok-pi can inspect what a session changed without inventing a separate review
application.

- `/review-session` opens a native changed-file list and BlockViewer diff.
- `/review-message` first selects the edit message to review.
- Press `t` in the review modal to switch flat files and a cwd-relative tree;
  press `r` to include or hide read-only file accesses. Both defaults live in F2.
- `/timeline` toggles the rail beside scrollback. `/timeline-config` selects the entries it shows, including Pi tools by registration name.
- **Pi tree file rollback** is default off and requires restart. When enabled, a
  bridge extension records raw preimages for Pi built-in `write` and `edit`, then
  offers file-only restore from SessionTree.

File rollback does not move Pi's conversation leaf and is not a full chat-state
rewind. Symlink and missing-snapshot cases fail closed.
