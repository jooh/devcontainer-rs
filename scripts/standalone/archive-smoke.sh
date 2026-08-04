#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <cargo-dist-archive>" >&2
  exit 2
fi

archive="$1"
if [[ ! -f "$archive" ]]; then
  echo "cargo-dist archive not found: $archive" >&2
  exit 2
fi
archive_dir="$(cd "$(dirname "$archive")" && pwd -P)"
archive="$archive_dir/$(basename "$archive")"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
smoke_script="$script_dir/smoke.sh"
if [[ ! -x "$smoke_script" ]]; then
  echo "standalone smoke harness not found or not executable: $smoke_script" >&2
  exit 2
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

case "$archive" in
  *.tar.gz)
    tar -xzf "$archive" -C "$tmp_dir"
    ;;
  *)
    echo "unsupported cargo-dist archive format: $archive" >&2
    exit 2
    ;;
esac

binaries=()
while IFS= read -r -d '' candidate; do
  binaries+=("$candidate")
done < <(find "$tmp_dir" -type f -name devcontainer -print0)

if [[ "${#binaries[@]}" -ne 1 ]]; then
  echo "expected exactly one devcontainer executable in $archive, found ${#binaries[@]}" >&2
  find "$tmp_dir" -type f -print >&2
  exit 1
fi

if [[ ! -x "${binaries[0]}" ]]; then
  echo "packaged devcontainer executable is not executable: ${binaries[0]}" >&2
  exit 1
fi

"$smoke_script" "${binaries[0]}"
