# Native Compatibility Dashboard

- Pinned upstream commit: `2d81ee3c9ed96a7312c18c7513a17933f8f66d41`
- Pinned spec commit: `c95ffeed1d059abfe9ffbe79762dc2fa4e7c2421`
- Command matrix source: `docs/upstream/command-matrix.json`
- Native parity inventory: `docs/upstream/parity-inventory.md`

## Current snapshot

- Declared upstream command paths present natively: `20/20`
- Upstream options with a native source reference in mapped Rust sources: `200/200`
- The parity inventory is a static source-evidence report. It is intended to identify obvious gaps and track drift, not to claim semantic parity by itself.

## Highest-Impact Gaps

- `build`: Native runtime now layers Features for image, dockerfile, and Docker Compose configs. Several upstream build flags are still unimplemented or are only partially honored.
- `read-configuration`: `--include-features-configuration` resolves local/published Feature sets natively, but still relies on fixture/manual manifests rather than full OCI resolution. Variable substitution support is still narrower than upstream.
- `outdated`: Backed by fixture/manual catalog data rather than real upstream registry resolution.
- `features`: Top-level command exists, but several subcommands still use local/offline substitutes rather than real OCI flows.

## Guardrails

- `cargo test --manifest-path cmd/devcontainer/Cargo.toml`
- `npm test`
- `node build/generate-cli-reference.js --check`
- `node build/generate-parity-inventory.js --check`
- `node build/generate-compatibility-dashboard.js --check`
- `node build/check-native-only.js`
- `node build/check-parity-harness.js`
- `node build/check-spec-drift.js`
- `node build/check-no-node-runtime.js`
