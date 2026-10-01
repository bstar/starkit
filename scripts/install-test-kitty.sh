#!/usr/bin/env bash
# Private, checksum-pinned test runtime; never installs into system directories.
set -euo pipefail
destination="${1:?Pass a private destination directory}"
mkdir -p "$destination"
case "$(uname -s)" in
  Linux)
    archive="$destination/kitty.txz"
    curl --fail --location --retry 3 -o "$archive" https://github.com/kovidgoyal/kitty/releases/download/v0.49.2/kitty-0.49.2-x86_64.txz
    echo "d573618b911e9c461bd421b96c13c74c7f1cb2f1ac9c327818d4ff84366cf5c6  $archive" | sha256sum --check
    mkdir -p "$destination/runtime"
    tar -xJf "$archive" -C "$destination/runtime"
    ;;
  Darwin)
    archive="$destination/kitty.dmg"
    curl --fail --location --retry 3 -o "$archive" https://github.com/kovidgoyal/kitty/releases/download/v0.49.2/kitty-0.49.2.dmg
    echo "e524b894145d89cb76b584e3a9691196ae56bfa31d51384a44aa774252b7cef7  $archive" | shasum -a 256 --check
    mount="$destination/mount"
    mkdir -p "$mount"
    hdiutil attach "$archive" -readonly -nobrowse -mountpoint "$mount"
    trap 'hdiutil detach "$mount"' EXIT
    ditto "$mount/kitty.app" "$destination/kitty.app"
    mkdir -p "$destination/runtime/bin"
    ln -s "$destination/kitty.app/Contents/MacOS/kitty" "$destination/runtime/bin/kitty"
    ln -s "$destination/kitty.app/Contents/MacOS/kitten" "$destination/runtime/bin/kitten"
    ;;
  *) echo "Unsupported test host" >&2; exit 1 ;;
esac
