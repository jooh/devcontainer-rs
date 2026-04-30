#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <standalone-binary-path> [--docker-path <path>] [--docker-compose-path <path>]" >&2
  exit 2
fi

binary="$1"
shift
if [[ ! -x "$binary" ]]; then
  echo "standalone binary not found or not executable: $binary" >&2
  exit 2
fi

engine_path="docker"
runtime_args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --docker-path)
      if [[ $# -lt 2 ]]; then
        echo "--docker-path requires a value" >&2
        exit 2
      fi
      engine_path="$2"
      runtime_args+=("--docker-path" "$2")
      shift 2
      ;;
    --docker-compose-path)
      if [[ $# -lt 2 ]]; then
        echo "--docker-compose-path requires a value" >&2
        exit 2
      fi
      runtime_args+=("--docker-compose-path" "$2")
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

assert_file_contains() {
  local file="$1"
  local expected="$2"
  if ! grep -Fq -- "$expected" "$file"; then
    echo "expected '$expected' in $file" >&2
    cat "$file" >&2
    exit 1
  fi
}

run_devcontainer() {
  local subcommand="$1"
  shift
  local command=("$binary" "$subcommand")
  if [[ ${#runtime_args[@]} -gt 0 ]]; then
    command+=("${runtime_args[@]}")
  fi
  command+=("$@")
  "${command[@]}"
}

container_id_from_json() {
  local file="$1"
  sed -n 's/.*"containerId":"\([^"]*\)".*/\1/p' "$file"
}

assert_lifecycle_markers() {
  local workspace="$1"
  shift
  local marker
  for marker in "$@"; do
    if [[ ! -f "$workspace/$marker" ]]; then
      echo "expected lifecycle marker $marker" >&2
      ls -la "$workspace" >&2
      exit 1
    fi
  done
}

if ! command -v "$engine_path" >/dev/null 2>&1; then
  echo "$engine_path is required for real-engine smoke" >&2
  exit 2
fi

tmp_dir="$(mktemp -d)"
image_workspace="$tmp_dir/image-workspace"
compose_workspace="$tmp_dir/compose-workspace"
container_ids=()

cleanup() {
  local container_id
  if [[ ${#container_ids[@]} -gt 0 ]]; then
    for container_id in "${container_ids[@]}"; do
      "$engine_path" rm -f "$container_id" >/dev/null 2>&1 || true
    done
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

mkdir -p "$image_workspace/.devcontainer"
cat >"$image_workspace/.devcontainer/devcontainer.json" <<'EOF'
{
  "image": "alpine:3.20",
  "workspaceFolder": "/workspace",
  "updateRemoteUserUID": false,
  "onCreateCommand": "printf on-create > /workspace/.on-create",
  "updateContentCommand": "printf update-content > /workspace/.update-content",
  "postCreateCommand": "printf ready > /workspace/.ready",
  "postStartCommand": "printf started > /workspace/.started",
  "postAttachCommand": "printf attached > /workspace/.attached"
}
EOF

run_devcontainer up --workspace-folder "$image_workspace" >"$tmp_dir/image-up.json"
assert_file_contains "$tmp_dir/image-up.json" '"outcome":"success"'
image_container_id="$(container_id_from_json "$tmp_dir/image-up.json")"
if [[ -z "$image_container_id" ]]; then
  echo "container id missing from up output" >&2
  cat "$tmp_dir/image-up.json" >&2
  exit 1
fi
container_ids+=("$image_container_id")

assert_lifecycle_markers "$image_workspace" .on-create .update-content .ready .started .attached

exec_output="$(run_devcontainer exec --workspace-folder "$image_workspace" /bin/cat /workspace/.ready)"
if [[ "$exec_output" != "ready" ]]; then
  echo "unexpected exec output: $exec_output" >&2
  exit 1
fi

run_devcontainer run-user-commands --workspace-folder "$image_workspace" >"$tmp_dir/image-run-user-commands.json"
assert_file_contains "$tmp_dir/image-run-user-commands.json" '"outcome":"success"'

run_devcontainer set-up --workspace-folder "$image_workspace" >"$tmp_dir/image-set-up.json"
assert_file_contains "$tmp_dir/image-set-up.json" '"outcome":"success"'

mkdir -p "$compose_workspace/.devcontainer"
cat >"$compose_workspace/.devcontainer/docker-compose.yml" <<'EOF'
services:
  app:
    image: alpine:3.20
    command: sh -c "while sleep 3600; do :; done"
EOF
cat >"$compose_workspace/.devcontainer/devcontainer.json" <<'EOF'
{
  "dockerComposeFile": "docker-compose.yml",
  "service": "app",
  "workspaceFolder": "/workspace",
  "updateRemoteUserUID": false,
  "onCreateCommand": "printf compose-on-create > /workspace/.compose-on-create",
  "updateContentCommand": "printf compose-update-content > /workspace/.compose-update-content",
  "postCreateCommand": "printf compose-ready > /workspace/.compose-ready",
  "postStartCommand": "printf compose-started > /workspace/.compose-started",
  "postAttachCommand": "printf compose-attached > /workspace/.compose-attached"
}
EOF

run_devcontainer up --workspace-folder "$compose_workspace" >"$tmp_dir/compose-up.json"
assert_file_contains "$tmp_dir/compose-up.json" '"outcome":"success"'
compose_container_id="$(container_id_from_json "$tmp_dir/compose-up.json")"
if [[ -z "$compose_container_id" ]]; then
  echo "compose container id missing from up output" >&2
  cat "$tmp_dir/compose-up.json" >&2
  exit 1
fi
container_ids+=("$compose_container_id")

assert_lifecycle_markers "$compose_workspace" \
  .compose-on-create \
  .compose-update-content \
  .compose-ready \
  .compose-started \
  .compose-attached

compose_exec_output="$(run_devcontainer exec --workspace-folder "$compose_workspace" /bin/cat /workspace/.compose-ready)"
if [[ "$compose_exec_output" != "compose-ready" ]]; then
  echo "unexpected compose exec output: $compose_exec_output" >&2
  exit 1
fi

run_devcontainer run-user-commands --workspace-folder "$compose_workspace" >"$tmp_dir/compose-run-user-commands.json"
assert_file_contains "$tmp_dir/compose-run-user-commands.json" '"outcome":"success"'

run_devcontainer set-up --workspace-folder "$compose_workspace" >"$tmp_dir/compose-set-up.json"
assert_file_contains "$tmp_dir/compose-set-up.json" '"outcome":"success"'
