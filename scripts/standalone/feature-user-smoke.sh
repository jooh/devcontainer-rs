#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || ! -x "$1" ]]; then
  echo "usage: $0 <standalone-binary-path> [engine]" >&2
  exit 2
fi
binary="$1"
engine="${2:-docker}"
workspace="$(mktemp -d)"
base_image="devcontainer-feature-user-base-$$"
feature_image="devcontainer-feature-user-result-$$"
cleanup() {
  "$engine" image rm -f "$feature_image" "$feature_image-base" "$base_image" >/dev/null 2>&1 || true
  rm -rf "$workspace"
}
trap cleanup EXIT
mkdir -p "$workspace/.devcontainer/root-feature"
cat > "$workspace/.devcontainer/root-feature/devcontainer-feature.json" <<'JSON'
{"id":"root-feature","version":"1.0.0","name":"Root feature"}
JSON
cat > "$workspace/.devcontainer/root-feature/install.sh" <<'SH'
#!/bin/sh
set -eu
test "$(id -u)" = 0
printf 'installed as root\n' > /usr/local/share/feature-root-marker
SH
cat > "$workspace/.devcontainer/Dockerfile" <<'DOCKERFILE'
FROM debian:bookworm-slim
RUN groupadd --gid 1234 developer && useradd --uid 1234 --gid 1234 --create-home developer
USER developer:1234
DOCKERFILE
"$engine" build --tag "$base_image" --file "$workspace/.devcontainer/Dockerfile" "$workspace"
for source in image dockerfile; do
  if [[ "$source" == image ]]; then
    source_config="\"image\":\"$base_image\""
  else
    source_config='"build":{"dockerfile":"Dockerfile"}'
  fi
  printf '{%s,"remoteUser":"root","features":{"./root-feature":{}}}\n' "$source_config" > "$workspace/.devcontainer/devcontainer.json"
  "$binary" build --workspace-folder "$workspace" --image-name "$feature_image" --docker-path "$engine" --omit-syntax-directive > "$workspace/result.json"
  "$engine" run --rm --entrypoint /bin/sh "$feature_image" -c 'test "$(id -u)" = 1234 && test "$(cat /usr/local/share/feature-root-marker)" = "installed as root"'
  actual_user="$("$engine" image inspect --format '{{.Config.User}}' "$feature_image")"
  if [[ "$actual_user" != developer:1234 ]]; then
    echo "expected developer:1234 after $source feature build, got $actual_user" >&2
    exit 1
  fi
done
echo "[feature-user] feature installation ran as root and preserved the image user."
