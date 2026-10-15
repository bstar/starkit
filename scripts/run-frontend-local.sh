#!/usr/bin/env bash
set -euo pipefail
script="${BASH_SOURCE[0]}"
while [[ -L "$script" ]]; do
  directory="$(cd -- "$(dirname -- "$script")" && pwd)"
  script="$(readlink "$script")"
  [[ "$script" = /* ]] || script="$directory/$script"
done
root="$(cd -- "$(dirname -- "$script")/.." && pwd)"
if [[ -f "$root/target/frontend-env.sh" ]]; then source "$root/target/frontend-env.sh"; fi
exec "$root/target/release/examples/session-frontend" "$@"
