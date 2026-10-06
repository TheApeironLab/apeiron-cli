---
name: apeiron-cli
description: Use Apeiron CLI to configure and deploy Apeiron through its local browser wizard and dispatch ontology or other configured application commands.
---

Install: clone TheApeironLab/apeiron-cli and run `bun install --frozen-lockfile`, then `bun link`.
Set APEIRON_ONTO_ROOT to the ontology checkout when it is not adjacent.
Authentication and endpoints follow each module's existing configuration.

Common commands:
- `apeiron init`: open setup for organization, deployment environment and apps, then run Helmfile sync.
- `apeiron init --no-open`: print the local wizard URL for manual browser access.
- `apeiron --help`: command table, schema=apeiron.v1.
- `apeiron status`: module, entry, available table.
- `apeiron verify`: check entry exists; exit 4 if missing.
- `apeiron onto status`: inspect ontology context.
- `apeiron onto schema`: inspect published ontology.

Recovery:
- Missing checkout: set APEIRON_ONTO_ROOT.
- Missing Bun: install Bun or set APEIRON_BUN_BIN.
- Unknown module: set APEIRON_<MODULE>_BIN to its executable.

The init wizard writes ~/.config/apeiron/config.json (or XDG_CONFIG_HOME/apeiron/config.json).
Use --config to change the destination. Keep it outside Git.
Setup defaults to K3s deployment (ubuntu profile), collecting the Chentu checkout, external YAML values and kubeconfig.
The collapsed 开发测试 section contains 本地测试（k3d）; enabling it reveals the Docker work directory/toolbox image and a persistent test-mode indicator. No CLI flag is needed. Saved native/local profiles are preserved.
Existing environment variables can prefill these fields. A prepared cluster, deployment tools, real images and external artifacts are required.
The last step starts Chentu's native run.sh sync (or the lab Docker wrapper). It does not provision hosts or clusters.
The model page is removed. Existing model declarations in the environment are preserved and validated by Chentu; legacy keys remain only on disk.
Never print saved credentials, kubeconfig contents, environment values or raw deployment logs in chat.
Each run writes a private environment copy and Helmfile log under deployments/run-*. The browser shows status, exit code and recognized progress only.
Required apps: Vasi, Apeiron (including Ops), Limani, Task, Corpus, Chat, Nexus and Mail.
Files and Gateway default to selected; Filer, Git, GPUStack, Langfuse and Grafana default to unselected.
Grafana controls Chentu's kps/Loki/Promtail observability component. Disabling an app does not uninstall existing releases.
Reloading the page resumes status polling without redeploying. Concurrent deployment of one source environment is locked.
After failure, inspect the private log locally, correct configuration, and retry sync; completed changes are not automatically rolled back.
Finish after deployment closes the server. Ctrl+C stops active deployment; closing a browser tab leaves it running.
Validation: bun run typecheck; bun test; bun run build; bun run test:ui (isolated fixture deployer).
For read-only real Helmfile integration: bun scripts/test-helmfile.ts /path/to/chentu toolbox-image.
