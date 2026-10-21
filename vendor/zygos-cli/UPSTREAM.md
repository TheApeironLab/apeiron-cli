# Zygos CLI source

Vendored from https://github.com/TheApeironLab/zygos at
`4df6ad081625c45d099599364d0b93d4be03ba86`, `apps/cli`.

The generated command/API catalogs are copied unchanged. `src/lib.rs` adds
`run_with_command` to prefix generated help/follow-up commands with `apeiron zygos`;
API payloads and full responses remain unchanged.
The manifest disables the standalone binary and aligns serde, serde_json,
and tokio patch versions with the host Apeiron workspace. The standalone
HTTP integration tests and Cargo.lock remain in the upstream repository;
this library uses Apeiron's root Cargo.lock. Library unit tests are retained.

When updating, refresh the library source and catalogs together from an exact
Zygos commit, update this reference, then run the embedded CLI tests.
