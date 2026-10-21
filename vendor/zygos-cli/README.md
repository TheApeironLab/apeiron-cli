# zygos-cli

Rust HTTP CLI and reusable library, maintained in the Zygos repository. The server owns all
business rules through Workspace + Schema. No Node, database credentials or object-store credentials
are needed to run the compiled CLI. A running Zygos API is required.

## Build and run

```bash
cargo build --release --locked --manifest-path apps/cli/Cargo.toml
apps/cli/target/release/zygos help --compact
apps/cli/target/release/zygos --base-url http://127.0.0.1:8000 ref list
```

`--base-url` defaults to `ZYGOS_API_BASE_URL`, then `http://127.0.0.1:8000`. For the offline reverse
proxy, supply `http://<host>/api`; the prefix is preserved. Flags may appear anywhere before `--`.
`--compact` selects compact JSON. `--timeout-seconds` defaults to 300 and includes streaming transfer.
The old `--database-url`, database/store environment variables and automatic root `.env` loading
are replaced by explicit API connection configuration. `help` requires no server.

```bash
zygos dataset ingest ./canonical.jsonl --ref demo --message "initial import"
zygos dataset ingest - --ref demo
zygos dataset show demo
zygos dataset records demo --limit 20
zygos dataset audit demo
zygos dataset export demo --inspect --converter canonical-jsonl
zygos dataset export demo --output ./demo.jsonl --converter canonical-jsonl
zygos converter list
zygos converter show canonical-jsonl
zygos transform list
zygos transform run subset --input demo --params '{"record_ids":["rec_<hex64>"]}' --ref demo-small
zygos ref list
zygos ref trash
zygos ref move demo <new-version> --expected-version <old-version>
zygos ref delete demo --use-current
zygos ref restore demo --use-current
zygos lineage show demo
zygos model list
zygos model show <model-id>
zygos model versions <model-id>
zygos model registration inspect --input request.json --output plan.json
zygos model registration commit --input request.json --expected-digest <hex64>
zygos model deployment list <version-id>
zygos model deployment check <version-id> <deployment-id>
```

Results go to stdout as JSON; raw export bytes are the exception. Error envelopes go to stderr.
Exit codes remain `0` success, `1` internal/unavailable, `2` bad input, `3` not found, `4` conflict
(including fidelity), `5` validation/resource limit. Interrupted binary commands exit `130`.

Import uploads stream from disk/stdin. Export inspects first and pins the immutable version; the API
validates the supplied `--accept-fidelity` digest. File output uses a mode-0600 temporary file beside
the destination, fsync and atomic replacement. Failure/cancellation removes the temporary file.
Redirects, ambient proxies and request retries are disabled. After an interrupted mutation, inspect
server state before retrying: cancellation cannot undo a request that has already committed.

## Use through Apeiron

Apeiron's existing Rust module dispatcher executes the same compiled Zygos binary, preserving
arguments, stdin/stdout/stderr and exit codes. No separate implementation of business commands is needed.
Configure its executable in the Agent process environment:

```bash
export APEIRON_ZYGOS_BIN=/absolute/path/to/zygos
export ZYGOS_API_BASE_URL=http://127.0.0.1:8000
apeiron zygos help --compact
apeiron zygos baml list
apeiron zygos baml show <template-id>
apeiron zygos baml create --help
apeiron zygos baml create --file template-create.json
apeiron zygos baml update <template-id> --file template-update.json
apeiron zygos baml publish <template-id> --expected-revision <revision> --dataset-version <dataset-version>
apeiron zygos baml archive <template-id> --expected-revision <revision>
apeiron zygos baml restore <template-id> --expected-revision <revision>
```

Use the absolute installed binary path (or this checkout's `apps/cli/dist/zygos` after building).
Keep that binary available when distributing Apeiron; the dispatcher does not install it automatically.
For a combined executable, embedding the Rust library below is also supported.

## Embed in Apeiron

Use a Cargo Git dependency pinned to a full commit containing the Rust migration:

```toml
[dependencies]
zygos-cli = { git = "https://github.com/TheApeironLab/zygos.git", rev = "<full-commit-sha>" }
```

Cargo discovers this package in `apps/cli`. The crate exposes `parse`, `Client` and `execute` for an
embedded command tree, and `run(&[String]) -> i32` for a complete adapter with process stdout/stderr.
Arguments exclude the executable name and the parent `apeiron zygos` prefix. The library does
not install signal handlers or exit the process; the standalone binary owns cancellation. Embedders
can cancel by dropping the execution future. Both entry points share `commands.json`.

Transport operation metadata is generated from the root `openapi.json` with
`node scripts/generate-rust-cli-contract.mjs`; check with `--check`. Domain wire schemas remain
owned by `@zygos/schema`; JSON is transported without a handwritten Rust schema mirror.

## Verify

```bash
cargo fmt --manifest-path apps/cli/Cargo.toml -- --check
cargo clippy --locked --manifest-path apps/cli/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path apps/cli/Cargo.toml
node scripts/generate-rust-cli-contract.mjs --check
# Requires an installed Apeiron and a release build of Zygos:
node scripts/check-apeiron-zygos.mjs
```

The HTTP tests use real temporary TCP listeners. The deployment `zygosctl` remains the Shell
operator wrapper for backup/restore/upgrade/rollback; changing its implementation is separate scope.
Its Model commands invoke this Rust CLI against the API running inside the same container.

## Named dataset CRUD and corrective errors

```bash
zygos dataset create demo
zygos dataset list
zygos dataset import demo --file records.jsonl
zygos dataset record add demo --file record.json
zygos dataset record show demo <record-id>
zygos dataset record update demo <record-id> --file edited-record.json --expected-version <dataset-version>
zygos dataset record delete demo <record-id> --expected-version <dataset-version>
zygos dataset archive demo
zygos dataset restore demo
```

`create` makes a named empty snapshot. `import` appends, it does not replace the dataset. `import`
and `record add` default to `--format draft` (`canonical-draft-jsonl-v1`); the server allocates IDs.
Use `--format canonical` for existing canonical records. `--file -` reads stdin. JSONL has one object
per physical line; record add/update accepts one JSON object, including pretty-printed JSON.

`zygos dataset --help` (also `dataset import --help` and `dataset record add --help`)
includes input rules, the default format, valid draft/canonical examples and a ready-to-save JSONL
line under `commands[0].input`. This help works offline and is generated from Schema → OpenAPI.
For example, with jq installed:

```bash
zygos dataset --help | jq -r '.commands[0].input.jsonl_example' > data.jsonl
zygos dataset create demo
zygos dataset import demo --file data.jsonl
```

Agents can also submit input directly. Validation errors on stderr include `error.detail.issues` (line,
JSON pointer, constraint and hint), `guidance.example` (a valid record), `issues_truncated` and
`committed:false`. Up to 32 issues are returned; syntax errors may stop scanning early. Correct the
reported fields and retry the whole input. No valid prefix is published from an invalid file.
The example uses `role: "user"`; response candidates use `role: "ai"`, not `"assistant"`.

`record update` replaces a **complete canonical record** (the `record` field from record show), not
its enclosing view or a partial patch. Metadata edits preserve the logical ID. Changing shared
contents/tools derives a new logical ID and records the exact parent revision. Read the new dataset's
records to find the resulting ID. All edits publish immutable versions; historical reads still work.
Use the `dataset_version` returned by record show as `--expected-version` to protect an edit prepared
over several requests. If omitted, the server uses the version read at the start of this operation.
Conflicts are not automatically retried. Archiving a dataset preserves its named ref, versions and files for restoration.

An exact import replay adds no duplicate records. An existing ID with different bytes is rejected:
use explicit record update. CAS or network failures may occur after an immutable snapshot was
published, so only input-validation failures claim `committed:false`; inspect state before retrying
other failures. A new draft with altered bytes may allocate new IDs.

### Training → model → deployment → evaluation

```bash
zygos training session create material-train-20261015
zygos training session list
zygos training session show SESSION_ID
zygos training session outputs SESSION_ID
zygos model artifact import --input artifact-import.json
zygos model artifact import-status IMPORT_ID
zygos model artifact list --registration-status unregistered
zygos model version show VERSION_ID
zygos model deployment create VERSION_ID --input deployment.json
zygos model deployment activate VERSION_ID DEPLOYMENT_ID
zygos model deployment check VERSION_ID DEPLOYMENT_ID
zygos evaluation list --model-version-id VERSION_ID
zygos evaluation show RUN_ID
```

`training session create` inspects the converter and pins its exact Dataset version. It does not
start a training process: configure and launch training in the native Studio at `/training`.
`ready` means the input is prepared. Import completion means the weights are archived; register
an Artifact-source Model Version with the existing registration inspect/commit flow afterward.
Deployment creation connects an already-running inference server; activation performs policy and
capability admission. It does not start or stop a GPU process.

For each JSON request, command-specific help includes `input_schema` generated from OpenAPI:

```bash
zygos model artifact import --help
zygos model deployment create --help
zygos evaluation create --help
```

Example `deployment.json` for an operator-configured private endpoint:

```json
{
  "display_name": "Spark decision model",
  "served_model_name": "qwen35-9b-material",
  "endpoint_base_url": "http://model-service:8000/v1",
  "connectivity_scope": "private_network",
  "auth_profile": "none",
  "credential_ref": null,
  "declared_capabilities": { "interfaces": ["chat_completions"], "context_limit": 2048 }
}
```

`evaluation create/start/complete/fail/cancel` record provider lifecycle events via the existing
REST contract; they do not run inference. Execute evaluations through the EvalScope UI/runtime,
then use list/show to inspect exact model, dataset, scores and archive state. Never mark a run
completed before its real evaluation finishes. `--input -` accepts request JSON from stdin.

### SSH GPU node

SSH node management is enabled by default; the API host does not need a GPU.
Set `ZYGOS_TRAINING_SSH_ENABLED=false` only to disable it explicitly.
Configure OpenSSH access on the API host, then use:

```bash
zygos training node check --input node.json
zygos training node connect --input node.json
zygos training node show
zygos training node disconnect
zygos training node connect --help
```

The API host runs OpenSSH using its existing key/agent and verified host keys. The CLI only calls
REST. `connect` prepares an isolated persistent remote Studio container and private SSH forwarding;
`disconnect` preserves training and files. See [SSH setup and lifecycle](../../docs/swift/SSH-TRAINING.md)
for the four configuration fields, remote prerequisites, and restart behavior.

### 保存训练节点

先用 `apeiron zygos training node list` 检查重名或重复主机。
`apeiron zygos training node create --input node.json` 仅保存配置，不连接或启动训练。
JSON 包含 `name` 与 `config: {host, image, model_path, workspace_path}`，不含密钥和密码。
环境检查使用 `training node check --input config.json`，输入仅为 config 对象。
失败或响应丢失后先 list，再决定是否重试。
