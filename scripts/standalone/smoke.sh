#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <standalone-binary-path>" >&2
  exit 2
fi

binary="$1"
if [[ ! -x "$binary" ]]; then
  echo "standalone binary not found or not executable: $binary" >&2
  exit 2
fi
binary_dir="$(cd "$(dirname "$binary")" && pwd -P)"
binary="$binary_dir/$(basename "$binary")"

assert_file_contains() {
  local file="$1"
  local expected="$2"
  if ! grep -Fq -- "$expected" "$file"; then
    echo "expected '$expected' in $file" >&2
    cat "$file" >&2
    exit 1
  fi
}

assert_file_not_contains() {
  local file="$1"
  local unexpected="$2"
  if grep -Fq -- "$unexpected" "$file"; then
    echo "did not expect '$unexpected' in $file" >&2
    cat "$file" >&2
    exit 1
  fi
}

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

workspace="$tmp_dir/workspace"
fake_bin="$tmp_dir/fake-bin"
log_dir="$tmp_dir/logs"
runtime_home="$tmp_dir/home"
runtime_tmp="$tmp_dir/tmp"
runtime_workdir="$tmp_dir/runtime-workdir"
mkdir -p \
  "$workspace/.devcontainer" \
  "$fake_bin" \
  "$log_dir" \
  "$runtime_home" \
  "$runtime_tmp" \
  "$runtime_workdir"

export HOME="$runtime_home"
export TMPDIR="$runtime_tmp"
cd "$runtime_workdir"

cat >"$workspace/.devcontainer/devcontainer.json" <<'EOF'
{
  "image": "alpine:3.20",
  "workspaceFolder": "/workspace",
  "updateRemoteUserUID": false,
  "onCreateCommand": "echo on-create",
  "updateContentCommand": "echo update-content",
  "postCreateCommand": "echo post-create",
  "postStartCommand": "echo post-start",
  "postAttachCommand": "echo post-attach"
}
EOF

cat >"$fake_bin/podman" <<'EOF'
#!/bin/sh
set -eu

LOG_DIR="${FAKE_PODMAN_LOG_DIR:?missing log dir}"
STATE_FILE="$LOG_DIR/container-created"
COMMAND="${1:-}"
shift || true
printf '%s %s\n' "$COMMAND" "$*" >> "$LOG_DIR/invocations.log"
printf 'cwd=%s\n' "$PWD" >> "$LOG_DIR/invocations.log"

case "$COMMAND" in
  image)
    if [ "${1:-}" = "inspect" ]; then
      printf 'vscode\nlinux/amd64\n'
    fi
    ;;
  build)
    dockerfile=""
    while [ "$#" -gt 0 ]; do
      if [ "$1" = "-f" ]; then
        dockerfile="${2:-}"
        break
      fi
      shift
    done
    if [ ! -f "$dockerfile" ]; then
      echo "UID update Dockerfile is not bundled: $dockerfile" >&2
      exit 1
    fi
    case "$dockerfile" in
      */devcontainer-update-uid-*/updateUID.Dockerfile) ;;
      *)
        echo "UID update Dockerfile is not in its generated build context: $dockerfile" >&2
        exit 1
        ;;
    esac
    grep -Fq 'ARG BASE_IMAGE' "$dockerfile"
    ;;
  run)
    : > "$STATE_FILE"
    echo "fake-container-id"
    ;;
  ps)
    if [ -f "$STATE_FILE" ]; then
      echo "fake-container-id"
    fi
    ;;
  exec)
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --workdir|--user|-e|--env)
          shift 2
          ;;
        --env=*)
          shift
          ;;
        -i)
          shift
          ;;
        *)
          break
          ;;
      esac
    done
    container_id="${1:-}"
    shift || true
    printf '%s %s\n' "$container_id" "$*" >> "$LOG_DIR/exec.log"
    if [ "${1:-}" = "/bin/echo" ]; then
      shift
      printf '%s\n' "$*"
    fi
    ;;
  inspect)
    printf '[{"Config":{"Labels":{}},"Mounts":[]}]'
    ;;
  push|start|rm)
    ;;
  *)
    echo "unsupported fake podman command: $COMMAND" >&2
    exit 1
    ;;
esac
EOF
chmod +x "$fake_bin/podman"

"$binary" --help >/dev/null
"$binary" --version >/dev/null
"$binary" read-configuration --help >/dev/null
"$binary" up --help >/dev/null
"$binary" build --help >/dev/null
"$binary" exec --help >/dev/null
"$binary" features --help >/dev/null
"$binary" templates apply --help >/dev/null
"$binary" upgrade --help >/dev/null

missing_engine_stderr="$tmp_dir/missing-engine.stderr"
if PATH="$fake_bin" FAKE_PODMAN_LOG_DIR="$log_dir" \
  "$binary" up --workspace-folder "$workspace" \
  >"$tmp_dir/missing-engine.stdout" 2>"$missing_engine_stderr"
then
  echo "expected missing-engine smoke to fail" >&2
  exit 1
fi
assert_file_contains "$missing_engine_stderr" "Container engine executable not found: docker"
assert_file_contains "$missing_engine_stderr" "--docker-path podman"
if grep -Fq "os error 2" "$missing_engine_stderr"; then
  echo "raw ENOENT leaked from missing-engine smoke" >&2
  cat "$missing_engine_stderr" >&2
  exit 1
fi

FAKE_PODMAN_LOG_DIR="$log_dir" \
  "$binary" up --docker-path "$fake_bin/podman" --workspace-folder "$workspace" \
  >"$tmp_dir/up.json"
assert_file_contains "$tmp_dir/up.json" '"outcome":"success"'
assert_file_contains "$tmp_dir/up.json" '"containerId":"fake-container-id"'

exec_output="$(
  FAKE_PODMAN_LOG_DIR="$log_dir" \
    "$binary" exec --docker-path "$fake_bin/podman" --container-id fake-container-id \
    --workspace-folder "$workspace" /bin/echo hello-from-artifact
)"
if [[ "$exec_output" != "hello-from-artifact" ]]; then
  echo "unexpected exec output: $exec_output" >&2
  exit 1
fi

FAKE_PODMAN_LOG_DIR="$log_dir" \
  "$binary" run-user-commands --docker-path "$fake_bin/podman" \
  --container-id fake-container-id --workspace-folder "$workspace" \
  >"$tmp_dir/run-user-commands.json"
assert_file_contains "$tmp_dir/run-user-commands.json" '"outcome":"success"'

FAKE_PODMAN_LOG_DIR="$log_dir" \
  "$binary" set-up --docker-path "$fake_bin/podman" \
  --container-id fake-container-id --workspace-folder "$workspace" \
  >"$tmp_dir/set-up.json"
assert_file_contains "$tmp_dir/set-up.json" '"outcome":"success"'

assert_file_contains "$log_dir/invocations.log" "run "
assert_file_contains "$log_dir/invocations.log" "ps -q"
assert_file_contains "$log_dir/invocations.log" "cwd=$runtime_workdir"
assert_file_contains "$log_dir/exec.log" "/bin/sh -lc echo on-create"
assert_file_contains "$log_dir/exec.log" "/bin/sh -lc echo update-content"
assert_file_contains "$log_dir/exec.log" "/bin/sh -lc echo post-create"
assert_file_contains "$log_dir/exec.log" "/bin/sh -lc echo post-start"
assert_file_contains "$log_dir/exec.log" "/bin/sh -lc echo post-attach"

# Exercise the UID update path in the published binary.
uid_workspace="$tmp_dir/uid-workspace"
mkdir -p "$uid_workspace/.devcontainer"
cat >"$uid_workspace/.devcontainer/devcontainer.json" <<'EOF'
{
  "image": "alpine:3.20",
  "remoteUser": "vscode"
}
EOF
: > "$log_dir/invocations.log"
rm -f "$log_dir/container-created"
FAKE_PODMAN_LOG_DIR="$log_dir" \
  "$binary" up --docker-path "$fake_bin/podman" --workspace-folder "$uid_workspace" \
  >"$tmp_dir/uid-up.json"
assert_file_contains "$tmp_dir/uid-up.json" '"outcome":"success"'
if [[ "$(uname -s)" == "Linux" ]]; then
  assert_file_contains "$log_dir/invocations.log" "build "
else
  assert_file_not_contains "$log_dir/invocations.log" "build "
fi
