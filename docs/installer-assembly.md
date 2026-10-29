# Maintainer installer assembly

`assemble-installer.js` combines an already extracted Chentu payload and an already built CLI release. It does not build, download, publish, tag, or deploy. Chentu owns payload generation and delivery contracts; this script owns the wizard wrapper. Requires Bun, bash, tar and Python 3 on the maintainer host.

```sh
bun scripts/assemble-installer.js \
  --bundle /absolute/chentu-package \
  --cli /absolute/cli-release \
  --candidate local-v7-wizard \
  --output /absolute/new-output \
  --profile /absolute/assembly-profile.json \
  --validator /absolute/installer-support \
  --plan
```

The CLI directory must contain `apeiron` and `build-provenance.json` from its existing release. Every payload regular file except its root `SHA256SUMS` must have a checksum entry. Symlinks, hard links, special files, traversal, duplicates, missing files and unlisted resources fail closed. Inputs and existing outputs are not modified. Failure after output reservation leaves an incomplete output for inspection; choose a new path for a later attempt.

The explicit profile contains `contract` (`legacy-native` or `hosted-first`), `config` (the default wizard configuration), and legacy supplements `expectedCli: {commit: "<full SHA>", binary: {size: 0, sha256: "..."}}` and `bundleSource: {commit: "<full SHA>", evidence: "..."}`. Legacy supplements are operator-supplied provenance, not inferred source authentication. Hosted declarations take precedence and conflicting supplement commits fail. Config must be offline. `legacy-native` only accepts native multi-k3s with documentation IP placeholders (192.0.2.x), empty SSH credentials and one server/two agents. `hosted-first` accepts all three topologies: multi-k3s with three placeholder servers (`ha: true`), single-k3s with `nodes: []` (the management machine is the node), or single-k3d with `nodes: []`, `entryIp: 127.0.0.1`, ports 54320/54321 and runner `docker`. Templates live in the publisher workspace (`integration/assembly-prep/profiles/{single-k3d,single-k3s,multi-k3s}.json`). The Rust developer validator (`installer-support`) picks the `k3d-amd64` catalog for single-k3d and `k3s-amd64` otherwise, enforces the matching hosted node rules, requires native operator tools and wheels only for K3s topologies, and reports the validated `topology`. Hosted base includes all declared base components regardless of the visible application selection.

Build `runtime/examples/installer-support.rs` against each release's exact CLI source. The adapter calls the real configuration, catalog, resource and distribution code, returns checked files and component closure, and must carry the same embedded source revision as the CLI. Do not use a newer parser to claim an older binary accepts a contract. This developer adapter is not a user-facing CLI subcommand.

`--plan` writes nothing and returns an inventory, default config, closure and input bindings. The payload is hashed in full. An optional `--wizard-proof /absolute/proof.json` contains `status: "passed"`, `page: true`, `configuration: true`, `buildInfo` from the actual binary's `--build-info` output, and `bindings` copied exactly from the plan. Those bind the complete payload inventory, CLI binary, profile, generated startup script and default config. Include the isolated Linux test procedure, page/configuration assertions, home-state and relocated/read-only package checks in that proof. The assembler verifies binding consistency; it does not turn assertions into independently executed smoke tests.

`--assemble` requires a bound passed wizard proof plus `runtimeToolExecution`, `pythonDependencyInstallation`, `relocation`, and `homeState`, each with `status: "passed"`. There is no skip switch. Replace `--plan` with `--assemble` to create:

```
new-output/
  candidate/{start.sh,bin/apeiron,bundle/,README.md,provenance/,validation.json,SHA256SUMS}
  candidate.tar
  archive-validation.json
  SHA256SUMS
```

Each candidate uses `~/.apeiron/installers/<candidate>` for state; `APEIRON_INSTANCE` (default `default`) selects `instances/<name>/config.json` and `APEIRON_WIZARD_PORT` (default 3210) the wizard port, so one package can host several instances. `start.sh` passes extra arguments (such as `--no-open`) to `apeiron chentu init` and the CLI prints the `url`/`config`/`forward` lines the acceptance harness reads. Startup only opens the wizard; deployment requires the user's action. Moving the package updates the existing config's bundle path. Host prerequisites are Ubuntu 22.04 AMD64, bash, ssh, tar and gpg; bundled Python is unpacked into the candidate's home state.

The archive verifier rereads every regular file without extracting, requires exact agreement with the internal checksum manifest, and produces a separately checksummed receipt. `validation.json` distinguishes assembly/resources/wizard from actual deployment, disconnected deployment, login and business acceptance. Missing wizard proof remains `not-run` in a plan and blocks assembly. Assembly does not repair v7's known hosted image-pin failure or establish any cluster acceptance.
