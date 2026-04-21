# Compose Parity Inventory

Pinned upstream CLI commit: `2d81ee3c9ed96a7312c18c7513a17933f8f66d41`

This is a semantic parity note for the native Rust Compose path. It complements the generated command-matrix inventory in `docs/upstream/parity-inventory.md`, which only records static source references.

## Matched on this branch

- Compose CLI discovery now prefers `docker compose version --short` and falls back to `docker-compose version --short`.
- `dockerComposeFile: []` now follows the upstream default-file search shape: `COMPOSE_FILE`, workspace `.env` `COMPOSE_FILE`, then `docker-compose.yml` plus optional `docker-compose.override.yml`.
- Compose `up` now matches upstream defaults more closely:
  - start all services when `runServices` is unset
  - append the primary service when `runServices` omits it
  - use `--no-recreate` when reusing or expecting an existing container
  - default the remote workspace folder to `/` when Compose config omits `workspaceFolder`
- Compose `up --mount ...` is now threaded into generated override files, with the same config-before-CLI ordering as the single-container engine path.
- Compose start overrides now preserve the first-file `version:` prefix, emit the keepalive wrapper entrypoint, merge feature/config `entrypoints`, honor `overrideCommand`, and declare named volumes at the top level.
- Compose build now accepts `--cache-from` by generating a build override file, and explicitly rejects `--cache-to`, `--output`, `--platform`, and `--push` in the same areas upstream rejects them.

## Remaining gaps

- Project-name `.env` lookup still diverges:
  - native code reads `.env` next to the first compose file in `cmd/devcontainer/src/runtime/compose/project.rs`
  - upstream reads `.env` from the caller working directory / workspace root when `dockerComposeFile` is empty
  - current native coverage still pins the old behavior in `cmd/devcontainer/tests/runtime_container_smoke/compose_project.rs`
- The native Compose path still derives service metadata from raw YAML files instead of `docker compose config`.
  - upstream resolves the merged Compose model first
  - native behavior can still diverge for profile expansion, env interpolation, multi-file merge edge cases, and custom-tag handling
- The keepalive wrapper preserves Compose-service `entrypoint`/`command`, but it does not inspect image `Entrypoint`/`Cmd` defaults the way upstream does.
  - configs that rely on image defaults rather than service-level overrides can still start differently
- Compose build still rejects `--label`.
  - upstream accepts the flag on `build`
  - native Compose build does not yet thread labels into a Compose build override

## Coverage added here

- `cmd/devcontainer/tests/runtime_container_smoke/compose_flow.rs`
- `cmd/devcontainer/tests/runtime_container_smoke/compose_project.rs`
- `cmd/devcontainer/tests/runtime_build_smoke/compose.rs`
- `cmd/devcontainer/src/runtime/compose/tests.rs`
