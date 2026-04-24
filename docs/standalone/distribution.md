# Native distribution

## Release artifacts

- GitHub Releases is the active distribution channel.
- `.github/workflows/devcontainer-release.yml` builds release archives for Linux x64 (glibc), Linux x64 (musl), macOS x64, and macOS arm64.
- Each release artifact currently includes a compressed archive and a SHA-256 checksum file.
- PyPI publishes the same native CLI as the `devcontainer-rs` package. Installing it exposes the `devcontainer` executable on `PATH`.

## PyPI install flow

PyPI is a convenience channel for enterprise environments that already approve Python package installation:

```bash
uv tool install devcontainer-rs
```

`pipx install devcontainer-rs` is also supported. The package ships platform-specific wheels with the Rust binary embedded; it does not provide a Python API and does not download a binary during installation or first run.

The initial PyPI wheel set matches the standalone release targets:

- Linux x64 glibc (`manylinux2014_x86_64`)
- Linux x64 musl (`musllinux_1_2_x86_64`)
- macOS x64
- macOS arm64

## Local build flow

- `scripts/standalone/build.sh <target>` builds the Rust release binary and places it under `dist/standalone/`.
- `scripts/standalone/build-linux-x64-musl.sh` builds the Linux x64 musl artifact for older-glibc distro compatibility.
- `scripts/standalone/smoke.sh <binary>` runs the repo-owned smoke commands against a built artifact.
- `make pypi-wheel-smoke` builds a local wheel with `uv`, installs it into an isolated environment, and runs the standalone smoke against the installed `devcontainer` executable.

## Current limitations

- The repository no longer ships or maintains the old bundled-Node installer path.
- Release automation does not currently sign artifacts or notarize macOS builds.
- PyPI publication requires a PyPI Trusted Publisher configured for `.github/workflows/devcontainer-release.yml` and the `pypi` GitHub environment.
- Compatibility tooling in `package.json` is not part of the runtime distribution path.
