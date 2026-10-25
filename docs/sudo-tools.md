# Apeiron CLI

The `apeiron` executable in this tools directory is the same CLI that launched
this agent. Run it with `--help`, then consult the selected module's help.
Use absolute paths when the executable is not on PATH. Keep the user's working
directory; changing into this tools directory is unnecessary.

Use native non-interactive commands and JSON output where supported. A missing
application or an unauthenticated session is an actionable error, not permission
to install services or obtain new credentials without the user's authorization.
Model-provider credentials do not grant platform access. Use the user's existing
platform authentication and the agent's normal permission checks.

`apeiron sudo` launches this agent. Use the other Apeiron modules to operate the
platform; do not recursively start agents to perform ordinary CLI operations.
Never print credentials or place them in command arguments.
