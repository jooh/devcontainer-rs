#!/usr/bin/env bash
set -euo pipefail

if [[ $# -gt 1 ]]; then
  echo "usage: $0 [wheel-path]" >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
smoke_script="$script_dir/smoke.sh"
if [[ ! -x "$smoke_script" ]]; then
  echo "standalone smoke harness not found or not executable: $smoke_script" >&2
  exit 2
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

wheel_dir="$tmp_dir/wheels"
venv_dir="$tmp_dir/venv"
if [[ $# -eq 1 ]]; then
  wheel="$1"
  if [[ ! -f "$wheel" ]]; then
    echo "wheel not found: $wheel" >&2
    exit 2
  fi
  wheel_parent="$(cd "$(dirname "$wheel")" && pwd -P)"
  wheel="$wheel_parent/$(basename "$wheel")"
else
  repo_root="$(cd "$script_dir/../.." && pwd -P)"
  mkdir -p "$wheel_dir"
  (
    cd "$repo_root"
    uv build --wheel --out-dir "$wheel_dir"
  )

  shopt -s nullglob
  wheels=("$wheel_dir"/devcontainer_rs-*.whl)
  shopt -u nullglob

  if [[ "${#wheels[@]}" -ne 1 ]]; then
    echo "expected exactly one devcontainer-rs wheel, found ${#wheels[@]}" >&2
    printf '%s\n' "$wheel_dir"/* >&2
    exit 1
  fi
  wheel="${wheels[0]}"
fi

uv venv "$venv_dir" >/dev/null
python_bin="$venv_dir/bin/python"
installed_binary="$venv_dir/bin/devcontainer"

uv pip install --python "$python_bin" --no-deps --only-binary :all: "$wheel"

if [[ ! -x "$installed_binary" ]]; then
  echo "installed devcontainer executable not found: $installed_binary" >&2
  exit 1
fi

expected_version="$(
  "$python_bin" -c 'from importlib.metadata import version; print(version("devcontainer-rs"))'
)"
actual_version="$("$installed_binary" --version)"
if [[ "$actual_version" != "$expected_version" ]]; then
  echo "expected devcontainer --version to print $expected_version, got $actual_version" >&2
  exit 1
fi

"$smoke_script" "$installed_binary"
