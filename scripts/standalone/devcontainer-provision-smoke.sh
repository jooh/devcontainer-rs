#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || ! -x "$1" ]]; then
  echo "usage: $0 <standalone-binary-path>" >&2
  exit 2
fi

binary="$1"
repository_root="$(git rev-parse --show-toplevel)"
tmp_dir="$(mktemp -d)"
workspace="$tmp_dir/workspace"
result_file="$tmp_dir/up.json"
container_id=""

cleanup() {
  if [[ -n "$container_id" ]]; then
    docker rm --force "$container_id" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required for the devcontainer provisioning smoke test" >&2
  exit 2
fi

git clone --quiet --local --no-hardlinks "$repository_root" "$workspace"

"$binary" up --workspace-folder "$workspace" >"$result_file"
if ! grep -Fq '"outcome":"success"' "$result_file"; then
  echo "devcontainer creation did not report success" >&2
  cat "$result_file" >&2
  exit 1
fi

container_id="$(sed -n 's/.*"containerId":"\([^"]*\)".*/\1/p' "$result_file")"
if [[ -z "$container_id" ]]; then
  echo "devcontainer creation did not return a container id" >&2
  cat "$result_file" >&2
  exit 1
fi

"$binary" exec --workspace-folder "$workspace" /bin/bash -lc '
  set -euo pipefail
  test -d node_modules
  git --version
  node --version
  corepack --version
  COREPACK_ENABLE_PROJECT_SPEC=0 corepack yarn --cwd upstream --version
  rustc --version
  cargo --version
  cargo deny --version
  cargo llvm-cov --version
  uv --version
'

echo "[devcontainer-provision] devcontainer built, provisioned, and exposed the required tools."
