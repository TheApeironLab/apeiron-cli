---
name: apeiron-cli
description: Use Apeiron CLI to configure a local Apeiron installation through its browser wizard and dispatch ontology or other configured application commands.
---

Install: clone TheApeironLab/apeiron-cli and run `bun install --frozen-lockfile`, then `bun link`.
Set APEIRON_ONTO_ROOT to the ontology checkout when it is not adjacent.
Authentication and endpoints follow each module's existing configuration.

Common commands:
- `apeiron init`: open the local wizard for slug, selected apps and LLM connection.
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
Use --config to change the destination. Keep it outside Git; it contains an API key.
Never print the saved file or request the real key in chat. Let the user enter it in the password field.
Existing keys are redacted in browser responses; leaving the field empty retains the key.
Init saves preferences only: it does not deploy, start applications or invoke a model.
Required apps: Vasi, Apeiron (including Ops), Limani, Task, Corpus and Chat. Files, Gateway and Nexus default to selected; Filer, Mail, Git, GPUStack, Langfuse and Grafana default to unselected. Optional choices persist across sessions; newly required apps are added only when the user saves.
Finish in the browser or Ctrl+C closes the local server.
