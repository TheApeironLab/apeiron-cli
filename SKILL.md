---
name: apeiron-cli
description: Use Apeiron CLI to dispatch ontology and configured product module commands.
---

Install: clone TheApeironLab/apeiron-cli and run `bun install --frozen-lockfile`, then `bun link`.
Set APEIRON_ONTO_ROOT to the ontology checkout when it is not adjacent.
Authentication and endpoints follow each module's existing configuration.

Common commands:
- `apeiron --help`: command table, schema=apeiron.v1.
- `apeiron status`: module, entry, available table.
- `apeiron verify`: check entry exists; exit 4 if missing.
- `apeiron onto status`: inspect ontology context.
- `apeiron onto schema`: inspect published ontology.

Recovery:
- Missing checkout: set APEIRON_ONTO_ROOT.
- Missing Bun: install Bun or set APEIRON_BUN_BIN.
- Unknown module: set APEIRON_<MODULE>_BIN to its executable.
