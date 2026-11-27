#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || ! -x "$1" ]]; then
  echo "usage: $0 <standalone-binary-path>" >&2
  exit 2
fi
binary="$1"
podman version
workspace="$(mktemp -d)"
container_ids=()
cleanup() {
  local container_id
  for container_id in "${container_ids[@]}"; do
    podman rm -f "$container_id" >/dev/null 2>&1 || true
  done
  rm -rf "$workspace"
}
trap cleanup EXIT

for spelling in equals separate; do
  folder="$workspace/$spelling"
  container_name="issue109-$$-$spelling"
  container_ids+=("$container_name")
  mkdir -p "$folder/.devcontainer"
  if [[ "$spelling" == equals ]]; then
    run_args="[\"--name\",\"$container_name\",\"--device=/dev/null:/dev/issue109-null:rw\"]"
  else
    run_args="[\"--name\",\"$container_name\",\"--device\",\"/dev/null:/dev/issue109-null:rw\"]"
  fi
  printf '{"image":"alpine:3.20","updateRemoteUserUID":false,"runArgs":%s}\n' "$run_args" > "$folder/.devcontainer/devcontainer.json"
  if ! "$binary" up --docker-path podman --workspace-folder "$folder" > "$folder/up.json"; then
    cat "$folder/up.json" >&2
    exit 1
  fi
  container_id="$(sed -n 's/.*"containerId":"\([^"]*\)".*/\1/p' "$folder/up.json")"
  if [[ -z "$container_id" ]]; then
    cat "$folder/up.json" >&2
    exit 1
  fi
  podman inspect --format '{{json .HostConfig.Devices}}' "$container_id" > "$folder/devices.json"
  if ! grep -Fq '/dev/issue109-null' "$folder/devices.json"; then
    cat "$folder/devices.json" >&2
    exit 1
  fi
  # shellcheck disable=SC2016
  "$binary" exec --docker-path podman --workspace-folder "$folder" /bin/sh -c \
    'test -c /dev/issue109-null && test "$(stat -c %t:%T /dev/issue109-null)" = 1:3 && printf device-test > /dev/issue109-null'
  echo "[podman-device] $spelling spelling passed inspect and in-container device checks"
done
