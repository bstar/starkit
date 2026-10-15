#!/usr/bin/env bash
# Build the presentation-only client and preserve its Nix audio plugin route.
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
exec nix --extra-experimental-features 'nix-command flakes' develop -c bash -c '
  set -euo pipefail
  cargo build --release --example session-frontend --no-default-features --features terminal-graphics,media
  if [[ -n "${ALSA_PLUGIN_DIR:-}" ]]; then
    printf "export ALSA_PLUGIN_DIR=%q\n" "$ALSA_PLUGIN_DIR" > target/frontend-env.sh
  else
    : > target/frontend-env.sh
  fi
'
