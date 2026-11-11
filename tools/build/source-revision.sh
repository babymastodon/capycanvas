#!/bin/sh
set -eu
cd "$(dirname -- "$0")/../.."
[ "$(git rev-parse --show-toplevel 2>/dev/null)" = "$(pwd -P)" ] || exit 0
commit=$(git rev-parse --short=9 HEAD 2>/dev/null) || exit 0
if git diff --quiet HEAD --; then printf '%s\n' "$commit"; else printf '%s*\n' "$commit"; fi
