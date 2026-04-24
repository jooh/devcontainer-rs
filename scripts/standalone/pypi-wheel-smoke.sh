#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

wheel_dir="$tmp_dir/wheels"
venv_dir="$tmp_dir/venv"
mkdir -p "$wheel_dir"

uv build --wheel --out-dir "$wheel_dir"

shopt -s nullglob
wheels=("$wheel_dir"/devcontainer_rs-*.whl)
shopt -u nullglob

if [[ "${#wheels[@]}" -ne 1 ]]; then
  echo "expected exactly one devcontainer-rs wheel, found ${#wheels[@]}" >&2
  printf '%s\n' "$wheel_dir"/* >&2
  exit 1
fi

uv venv "$venv_dir" >/dev/null
python_bin="$venv_dir/bin/python"
installed_binary="$venv_dir/bin/devcontainer"

uv pip install --python "$python_bin" --no-deps --only-binary :all: "${wheels[0]}"

if [[ ! -x "$installed_binary" ]]; then
  echo "installed devcontainer executable not found: $installed_binary" >&2
  exit 1
fi

expected_version="$(sed -nE 's/^version = "([^"]+)"/\1/p' cmd/devcontainer/Cargo.toml | head -n 1)"
actual_version="$("$installed_binary" --version)"
if [[ "$actual_version" != "$expected_version" ]]; then
  echo "expected devcontainer --version to print $expected_version, got $actual_version" >&2
  exit 1
fi

./scripts/standalone/smoke.sh "$installed_binary"
