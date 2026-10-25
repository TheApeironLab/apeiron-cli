# `apeiron sudo`

`apeiron sudo` runs the bundled Sudocode (`scode`) terminal agent. It requires
neither a Kubernetes cluster, Gateway release, nor Apeiron Backend. Model traffic
goes directly to your configured provider. The name is an Apeiron subcommand;
it does not invoke the system `sudo` or elevate privileges.

```sh
apeiron sudo
apeiron sudo --help
apeiron sudo --version
apeiron sudo --auth api-key --model minimal
apeiron sudo -p "Inspect this project" --output-format json
```

Arguments after `sudo` go to scode unchanged. The process retains its working
directory, terminal, stdin/stdout/stderr, permissions and signals. JSON output
contains no Apeiron banners. Sudocode owns sessions, tools and permission checks.

## Model configuration

The default global configuration is `~/.apeiron/sudo/`. Set
`SCODE_GLOBAL_CONFIG_DIR` to use another directory, including an existing
standalone Sudocode installation. The legacy `SUDO_CODE_CONFIG_HOME` override is
also honored. Project configuration follows Sudocode's native rules.

Create `sudocode.json` in that global directory. For an OpenAI Chat Completions
compatible endpoint, a minimal example is:

```json
{
  "auth_modes": {
    "api-key": {
      "openai": {
        "baseUrl": "https://models.example.internal/v1",
        "apiKeyEnv": "APEIRON_MODEL_API_KEY"
      }
    }
  },
  "models": {
    "minimal": {
      "alias": "minimal",
      "name": "My model",
      "providers": {
        "api-key": {
          "provider": "openai",
          "model": "YOUR_MODEL_ID",
          "api": "openai-completions"
        }
      }
    }
  }
}
```

Supply `APEIRON_MODEL_API_KEY` through your private environment or secret
manager, then run `apeiron sudo --auth api-key --model minimal`. Use the provider
protocol matching your endpoint. Existing configuration and credential files
are never rewritten by the launcher. A model connection alone does not grant
access to platform services.

## Calling Apeiron from the agent

On a normal launch, Apeiron registers its executable and usage documentation in
Sudocode's native `cli-tools/apeiron/tools/` catalog under the selected global
config directory. The shell PATH also includes the current Apeiron executable's
directory. The agent can inspect help and call ordinary Apeiron modules using
the user's existing service credentials. `--help` and `--version` do not create
this configuration. App modules still require their corresponding services;
the local AI agent itself does not depend on platform SSO.

## Runtime delivery and updates

Official release builds embed the target platform's compressed scode binary. First use
verifies the archive and executable SHA-256 and installs it atomically under
`~/.apeiron/runtimes/sudocode/<binary-sha256>/scode`; subsequent launches verify
the cached executable. Installation and launch need no runtime download. Model
inference still requires access to the selected endpoint.

`release/sudocode-runtime.json` pins all four macOS/Linux ARM64/x64 assets and
must match `release/recipe.json`'s Sudocode version and source commit. The build
fetches assets only in the external preparation environment, or reads a prepared
directory via `APEIRON_SCODE_ARCHIVE_DIR`. Both paths verify the same reviewed
digests. Users receive these bytes through Apeiron's normal OSS distribution.
The MIT notice is included in the archive’s `README.md`.

Automatic Sudocode update checks are disabled for this managed runtime. Upgrade
through the Apeiron installer; do not modify its cached executable. Both
Sudocode version and source commit appear in `apeiron --build-info`; archive
and executable digests also appear in release provenance.

For development, set `APEIRON_SCODE_BIN` to an absolute, trusted scode executable
to bypass the bundled runtime, or build with `APEIRON_SCODE_ARCHIVE` pointing to
the exact pinned archive for your platform. These inputs and Cargo outputs must
remain outside the checkout. Builds without a payload report an actionable
error on `sudo`; other Apeiron commands remain available. Official builds use
`bun scripts/build-release.js /external/output <target>` and require a clean
committed checkout and a verified runtime archive.

## Validation

`test/sudo.test.js` checks argument/stdio/exit/signal forwarding, configuration
isolation, CLI discovery, and publication pin checks without model calls.
`cargo test --locked` checks pin alignment and extraction integrity.
The native release smoke check executes the embedded `scode --version` from a
fresh home directory with a minimal PATH on each supported build platform.
Interactive terminal behavior should also be checked with `apeiron sudo` on
the native target before release.
