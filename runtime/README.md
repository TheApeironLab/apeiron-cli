# apeiron (CLI)

Run the apeiron agent on your own machine.

A laptop has no address apeiron can reach, so this dials out: one WebSocket to
the control plane, authenticated with a Personal Access Token, and the user
that token belongs to gets their agent *here* — with their files, their tools,
their model credentials, their `~/.nexus/sudocode`.

```sh
apeiron connect --server https://api.apeironlab.cn --token apeiron_sk_…
```

That is the whole setup: it installs itself as a background service and
attaches this machine. Everything else has a default:

| flag | default | |
| --- | --- | --- |
| `--root` | `~/.apeiron/workspaces` | where session directories live |
| `--agent` | `scode acp` | the agent to spawn |
| `--config-home` | `~/.nexus/sudocode` | where Plugins are projected |
| `--verbose` | off | log every ACP message and operation |

Each has an `APEIRON_*` environment variable (see `--help`). `--token`
has a companion `--token-file`, which is how the background service below
keeps the credential out of unit files and process listings.

The usual way in is the app: 设置 → 本地电脑 → 连接本机 hands you a
`curl … | sh` command with a freshly minted token.

## As a background service

```sh
apeiron connect --server https://api.apeironlab.cn --token apeiron_sk_…   # attach
apeiron status                                                            # attached?
apeiron disconnect                                                        # detach
apeiron serve --server … --token …                                        # run here instead
```

`connect` registers the same binary with the platform's own supervisor —
launchd (`~/Library/LaunchAgents/cn.apeironlab.apeiron.plist`) on macOS,
systemd `--user` (`~/.config/systemd/user/apeiron.service`) on Linux —
so it starts at login and survives closing the terminal. It needs no root: the
agent runs as the person whose files it exposes. The one-command installer
does this by default; pass `--foreground` to it to run `serve` in the terminal
instead. Two verbs, no overlap: `connect` sets the machine up to run in the
background, `serve` runs it here and installs nothing — which is also what the
unit's ExecStart points at, because a service whose start command was
`connect` would reinstall itself on every boot.

Worth knowing:

- The token goes to `~/.config/apeiron/token` (0600), not into the unit
  file, and re-running `connect` replaces it — that is what reconnecting from
  the app does.
- Logs go to `~/.local/state/apeiron/apeiron.log`.
- A crash restarts; a *clean* exit does not. Disconnecting from the app
  deletes the token, this exits 0 and removes its own service — so
  "disconnect" means disconnected, not "back in one second". Only the service
  removes the service: a `serve` you started by hand with `--token` exits and
  leaves the installed one alone, because revoking one credential says
  nothing about the other.
- On Linux, staying up while you are logged out needs
  `sudo loginctl enable-linger $USER`; the installer says so.
- Where neither supervisor exists (a container, WSL1), `connect` says so and
  points at `--foreground`; it leaves no token file behind on that failure.
- On macOS the binary is installed inside `~/.apeiron/Apeiron.app`, and
  `~/.apeiron/bin/apeiron` is a symlink into it. macOS draws an icon only for
  an `.app` bundle, so without one the background service appears in
  系统设置 → 通用 → 登录项与扩展 as an unnamed grey `exec` block. `connect`
  writes the bundle's path into the unit — `current_exe()` is canonicalised,
  so running it through the symlink still registers the bundle. The LaunchAgent's
  `AssociatedBundleIdentifiers` attributes it to Apeiron instead of the signing
  certificate owner. Releases sign the complete app; installation preserves
  its Info.plist, resources and signature unchanged.
  `cli/assets/make-icns.sh` rebuilds the icon from the brand logo; the
  generated `apeiron.icns` is committed, so a release build needs nothing extra.

## What it needs

`scode` on PATH, already configured with your model credentials. Nothing else
— no runtime, no container, no inbound port.

## What it does

Three things, and deliberately nothing more:

1. spawns `scode acp` and shuttles whole JSON-RPC messages between its stdio
   and the socket;
2. answers filesystem primitives — resolve a path, list a directory, read
   bytes, write a file, make directories;
3. runs the Plugin projector's shell commands.

Every rule *about* those primitives — what counts as inside a session, what
MIME a file is, how a colliding upload is renamed — is decided by the control
plane, against the resolved paths this program reports. That is why the binary
does not need reshipping when a rule changes, and why there is only ever one
implementation of the rules. The protocol is specified in
`backend/src/local/protocol.ts`; `src/protocol.rs` mirrors it.

## Guarantees, and their limits

- **Paths are fenced.** Every primitive refuses a path outside `--root` and
  `--config-home`, lexically, before anything is opened.
- **The environment is fenced.** The control plane's variables are merged over
  this machine's own, minus the names that choose which code runs (`PATH`,
  `HOME`, `LD_*`, `DYLD_*`, `NODE_OPTIONS`, …): apeiron may configure the
  agent, never hijack execution here.
- **But the agent is not sandboxed.** `scode` runs as you, with your
  permissions — that is the point of running it here rather than in a pod, and
  it means a prompt can do anything you can. Point `--root` somewhere
  disposable if that is not what you want.
- **`proc.run` executes what the control plane sends.** It exists for Plugin
  projection. A control plane you do not trust should not get a token.

## Lifecycle

A dropped socket is routine — a closed lid, a changed network — and it
reconnects with backoff. The agent process dies with the connection: apeiron
creates fresh sessions on every connect, so a child kept alive across a broken
tunnel would only be an orphan. Whatever run was in flight is lost, and the app
says so.

One machine per user. Connecting a second one replaces the first.

Two credential answers are final rather than retried: close code 4006 (the
token was deleted while connected) and a 401/403 at the handshake. It exits
instead of hammering a door that will not open, and takes its service
with it.

## Development

```sh
cargo build --release          # target/release/apeiron, ~1 MB
deploy/ci/check.sh cli         # fmt, clippy, tests, release build
```

The backend's `test/local-runtime.test.ts` drives this binary against a real
control plane; it skips if `target/release/apeiron` has not been built.

`deploy/local/two-runtimes/` runs both halves on one machine — apeiron in
Docker, this in your shell — with a smoke script that proves the loop.
