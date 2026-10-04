#!/usr/bin/env bash
#
# Packages one release archive for pmpx.
#
#     scripts/package-release.sh <target-triple> [out-dir]
#
# and it writes
#
#     dist/pmpx-<target>.tar.gz
#
# The archive holds exactly two entries, both at the top level: the binary (`pmpx`) and
# `LICENSE`. The layout is an interface, not a preference -- `pmpx self update` opens the
# archive for its own target and picks the binary out of it by name, and MIT requires the
# licence notice to travel with a copy of the binary anyway.
#
# The name is an interface too: `pmpx-<target>` with `.tar.gz` on Unix and `.zip` on
# Windows is what `self update` and the install script build their URLs from.
#
# This script is what CI runs, so a release can be reproduced locally with one command.

set -euo pipefail

if [ $# -lt 1 ] || [ $# -gt 2 ]; then
  echo "usage: $0 <target-triple> [out-dir]" >&2
  exit 2
fi

target="$1"
out_dir="${2:-dist}"
root="$(cd "$(dirname "$0")/.." && pwd)"

# `--locked`: a release is built from the committed lockfile, so the same tag yields the
# same dependency set no matter who builds it.
cargo build --release --locked --manifest-path "$root/Cargo.toml" --target "$target"

binary="$root/target/$target/release/pmpx"
if [ ! -x "$binary" ]; then
  echo "no executable at $binary" >&2
  exit 1
fi

mkdir -p "$out_dir"
archive="$out_dir/pmpx-$target.tar.gz"

# Staged in a temporary directory so the archive carries no path components of its own:
# `tar -C <dir> pmpx LICENSE` stores them as `pmpx` and `LICENSE`.
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
cp "$binary" "$staging/pmpx"
cp "$root/LICENSE" "$staging/LICENSE"

tar -czf "$archive" -C "$staging" pmpx LICENSE

echo "$archive"
