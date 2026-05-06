#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <target>" >&2
  echo "example targets: linux-x64, linux-x64-musl, linux-arm64, linux-arm64-musl, darwin-x64, darwin-arm64" >&2
  exit 2
fi

target="$1"
output="dist/standalone/devcontainer-${target}"
crate_manifest="cmd/devcontainer/Cargo.toml"

rust_target=""
case "$target" in
  linux-x64)
    ;;
  linux-x64-musl)
    rust_target="x86_64-unknown-linux-musl"
    ;;
  linux-arm64)
    rust_target="aarch64-unknown-linux-gnu"
    ;;
  linux-arm64-musl)
    rust_target="aarch64-unknown-linux-musl"
    ;;
  darwin-x64|darwin-arm64)
    ;;
  *)
    echo "unsupported target: $target" >&2
    exit 2
    ;;
esac

mkdir -p dist/standalone

if [[ -n "$rust_target" ]]; then
  cargo build --release --target "$rust_target" --manifest-path "$crate_manifest"
  binary_path="cmd/devcontainer/target/${rust_target}/release/devcontainer"
else
  cargo build --release --manifest-path "$crate_manifest"
  binary_path="cmd/devcontainer/target/release/devcontainer"
fi

cp "$binary_path" "$output"
chmod +x "$output"
