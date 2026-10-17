# Maintainer installer validation

`runtime/examples/installer-support.rs` is a build-time adapter, not a shipped CLI
subcommand. It reuses configuration validation, resource catalog parsing, dependency
planning, offline file verification, and release preparation from the actual CLI.

Build in a checkout matching the input CLI release source:

```sh
cargo build --locked --example installer-support
target/debug/examples/installer-support --bundle /absolute/payload --config /absolute/config.json
```

The payload directory contains `chentu/setup/install.json` and its resource files.
The configuration is the same JSON accepted by the browser wizard. The adapter
requires Ubuntu 22.04 AMD64 and an offline configuration: legacy uses multi-K3s with
one server and two agents and the seven standard applications; hosted accepts
multi-K3s with three servers, single-K3s without nodes, or single-K3d (docker runner,
127.0.0.1, 54320/54321) and retains all base components resolved by the catalog,
even if the wizard selects fewer apps. Single-K3d validates against the `k3d-amd64`
catalog and does not require native operator tools or wheels.

JSON output distinguishes requested applications, mandatory base components, and
the resolved component closure. `expectedCli` comes from the hosted manifest; legacy
packages require an explicit external assembly provenance declaration. The assembler
must check source identity and binary size/hash itself. `runtimeTools` lists the
verified Python archive, deployment binaries, and Chentu/Ansible wheels required by
`native_tools`. This establishes the required files, not successful tool execution
or transitive Python dependency installation. The assembler must install the wheels
offline in an isolated environment and verify tool execution separately; these
checks remain `not-run` in the adapter's output.

The static wheel check requires the Ansible version pinned in
`chentu/bootstrap/requirements.txt`, while allowing other declared wheel versions.
The hosted contract additionally requires `bin/crane`. Availability of a pinned
wheel does not prove that a runtime invocation without a version constraint selects
that version.

For an older source revision, copy this adapter and expose `app::config` as
`pub(crate)` in `runtime/src/app/mod.rs`. Record both adapter hash and this visibility
change; do not change the release's configuration/resource/distribution implementation.
An uncommitted adapter produces `validator.sourceClean = "false"`, and must never be
presented as a clean release rebuild. Record the exact parser source hashes alongside
the original release binary's clean build provenance.

Use a separate Cargo target directory for each source revision. Reusing one target
directory across sibling worktrees can reuse a build script's embedded source
identity; inspect the validator's reported revision before accepting its result.

The a792ec1 parser intentionally rejects native hosted packages. V7 must be checked
with its a1b1368 parser. That commit's `distribution::validate` accepts native
profiles, but `distribution::prepare` still admits only macOS ARM64 managers. The
actual Linux AMD64 check therefore fails before deployment. Preserve that failure;
the adapter must not bypass it to assemble a seemingly compatible v7 installer.
Calling only `distribution::validate` would skip platform admission and source
binding and is insufficient.

Validation only reads resources. It neither contacts nodes nor launches deployment.
Deployment, actual disconnected execution, login and business acceptance remain
`not-run`, independently of a successful resource check.
