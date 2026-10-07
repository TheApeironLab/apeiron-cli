---
name: apeiron-cli
description: Use Apeiron CLI to configure and deploy Apeiron through its local browser wizard and dispatch ontology or other configured application commands.
---

Install the standalone binary using the verified installer documented in README.md and docs/releases.md. No Bun is required for `apeiron init`. Source development uses Bun 1.3.14.

Common commands:
- `apeiron init`: open the six-step local setup: environment, organization, apps, deployment, access configuration, tests.
- `apeiron init --no-open --port 3210`: headless setup via SSH port forwarding.
- `apeiron --version`: published binary version.
- `apeiron --help`: command table, schema=apeiron.v1.
- `apeiron status`: module, entry, available table.
- `apeiron verify`: check configured module entry exists; exit 4 if missing.
- `apeiron onto status`: ontology context; configure APEIRON_ONTO_ROOT or APEIRON_ONTO_BIN as needed.

The wizard writes ~/.config/apeiron/config.json (or XDG_CONFIG_HOME/apeiron/config.json). Use --config to change the destination; keep it outside Git.
Online deployment downloads the pinned Chentu release and checksum-verified resources for selected apps. Offline mode requires a full bundle directory on the CLI host and performs no resource downloads. A CLI binary for a platform does not imply a matching Chentu deployment target exists. Current published Chentu supports K3d ARM64; Docker is required. See docs/install-package.md.

Required apps: Nexus, Vasi, Limani, Apeiron (including Ops).
Optional, selected by default: Task, Corpus, Chat, Files, Mail.
All other apps default to unselected. Unsupported bundle components are rejected before cluster changes. Disabling an app does not uninstall existing releases.

Never print credentials, kubeconfig contents, environment values or raw deployment logs in chat. Each deployment writes a private run directory. The page can display/download its log. Initial admin credentials are available only in step 6 after successful deployment, hidden by default.

Stopping deployment terminates its local processes, not completed Kubernetes changes. Retry checks resources and Helm pending states, then reruns sync; it is not checkpoint resume. Reloading the page resumes status polling without redeploying. Closing the tab leaves deployment running; Ctrl+C stops the wizard.

Validation: bun run typecheck; bun test; bun run test:ui. Release build and platform smoke tests are documented in docs/releases.md. Real deployment changes need separate target-specific acceptance; fixture tests do not prove cluster readiness.
