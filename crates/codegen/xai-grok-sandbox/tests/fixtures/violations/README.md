# Violation stderr fixtures

Table-driven inputs for `command::violation::coarse` (`coarse_tests.rs`). One file per captured
command; the test names the expected `Blocked` for each. Fixture paths use the fixture root
`/opt/ws-fixture/` so no capture carries a real user's home directory: a personal home directory in a
capture is written `/opt/ws-fixture/homedir`, the scratch workspace is
`/opt/ws-fixture/homedir/w1-scratch/ws` (the table's cwd), the fake home of the escape battery is
`/opt/ws-fixture/fakehome`. Nothing else in a capture is changed.

## `macos/` — real Seatbelt captures

Verbatim stderr captured on macOS 26.6.2 (`sandbox-exec`, default policy). Where the capture
elides a path (`…`) the fixture spells the path the row's command names; where the capture
summarises a tool's output (`gaierror` traceback) the fixture is that tool's standard text for the
errno it names.

| file | command |
|---|---|
| `ls-ssh-eperm` | `/bin/ls ~/.ssh` |
| `cat-ssh-eperm` | `cat …/fakehome/.ssh/id_rsa` |
| `sh-redirect-relative` | `sh -c 'echo hi > ../outside.txt'` |
| `touch-git-hooks` | `touch $WS/.git/hooks/x` |
| `mkdir-ws-grok` | `mkdir $WS/.grok` (absent) |
| `mv-rename-root` | `mv $WS ~/w1-scratch/ws2` |
| `curl-resolve-off` | `curl https://example.com`, network off |
| `bash-dev-tcp-connect` | `exec 3<>/dev/tcp/1.1.1.1/80` |
| `curl-tunnel-403` | `curl` through the proxy, host refused |
| `curl-noproxy-ip` | `curl --noproxy '*' https://1.1.1.1/` |
| `bash-nested-home` | `bash -c 'bash -c "echo nested > ~/x"'` |
| `sandbox-exec-setuid` | `sudo -n true` (exit 71) |
| `bash-kill-outside` | `kill -0 <outside pid>` |
| `ls-symlink-relative` | `ln -s ~/.ssh link && ls link/` |
| `pip-user-eperm` | `pip3 install --user requests`, `PYTHONUSERBASE=~/w1-scratch/pyuser` |
| `git-config-local` | `git config user.name evil` |
| `git-config-global-lock` | `git config --global user.name pwned` |
| `mkdir-config-git` | `mkdir -p ~/.config/git` |
| `chmod-setuid-copy` | `chmod 4755 ./idcopy` |
| `python-getaddrinfo` | `python3 -c 'socket.getaddrinfo(...)'` |
| `seatbelt-deny-report` | the unified-log `deny(1) file-write-create <path>` line (the one Seatbelt shape that is a marker) |

## `transcribed/` — tool shapes not captured

Node's `EPERM … open '<path>'`, npm's `EPERM` block, python's `[Errno 1] … '<path>'` and git's
`unable to create file .git/hooks/pre-commit` are transcribed from those tools' source, not
captured; they pin the quoted-token rule and the dot-directory-relative join. Replace with a real
capture when one is taken; keep the file names.

## `negative/` — "permission denied" that is not this sandbox

Texts that must never yield a card; `NEGATIVE_CASES` in `coarse_tests.rs` lists them.
