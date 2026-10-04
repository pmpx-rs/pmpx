#!/bin/sh
#
# Installs pmpx from a GitHub release.
#
#     curl -LsSf https://raw.githubusercontent.com/pmpx-rs/pmpx/main/install.sh | sh
#
# Environment:
#
#   PMPX_INSTALL_DIR   where the binary goes               (default: ~/.local/bin)
#   PMPX_VERSION       install this version                (default: the newest release)
#   PMPX_BASE_URL      download from somewhere else        (default: the GitHub release page;
#                      a mirror, and what the tests point at a local server)
#
# It writes exactly one file and touches nothing else: no shell profile, no PATH, no
# package manager. If the directory it writes to is not on PATH, it says so and stops.
#
# The download is verified against the release's SHA256SUMS before anything is unpacked. So
# is the version it reports afterwards, which catches a mirror serving a stale archive under
# a newer tag.

set -eu

REPO="pmpx-rs/pmpx"
BIN="pmpx"

die() {
  printf 'install.sh: %s\n' "$*" >&2
  exit 1
}

note() {
  printf '%s\n' "$*"
}

have() {
  command -v "$1" >/dev/null 2>&1
}

# ---------------------------------------------------------------------------
# Which release asset is this machine?
# ---------------------------------------------------------------------------

# The four published targets are the four common desktop ones. Anything else is not an
# error in the script: it is a platform with no prebuilt archive, and cargo is the answer.
target() {
  os=$(uname -s 2>/dev/null || echo unknown)
  arch=$(uname -m 2>/dev/null || echo unknown)

  case "$arch" in
    x86_64 | amd64) arch=x86_64 ;;
    arm64 | aarch64) arch=aarch64 ;;
    *) die "no prebuilt pmpx for architecture $arch; use 'cargo install pmpx'" ;;
  esac

  case "$os" in
    Linux)
      if [ "$arch" != x86_64 ]; then
        die "no prebuilt pmpx for Linux $arch; use 'cargo install pmpx'"
      fi
      printf 'x86_64-unknown-linux-gnu'
      ;;
    Darwin)
      printf '%s-apple-darwin' "$arch"
      ;;
    *)
      die "no prebuilt pmpx for $os; use 'cargo install pmpx'"
      ;;
  esac
}

# ---------------------------------------------------------------------------
# Talking to the release page
# ---------------------------------------------------------------------------

download() { # <url> <destination>
  if have curl; then
    curl -fsSL -o "$2" "$1"
  elif have wget; then
    wget -qO "$2" "$1"
  else
    die "neither curl nor wget is installed"
  fi
}

# The newest tag, read out of the `/releases/latest` redirect. That needs no JSON parsing
# and does not touch the API's rate limit.
latest_version() {
  url=""

  if have curl; then
    url=$(curl -fsSL -o /dev/null -w '%{url_effective}' \
      "https://github.com/$REPO/releases/latest" 2>/dev/null) || url=""
  elif have wget; then
    url=$(wget --server-response --spider "https://github.com/$REPO/releases/latest" 2>&1 |
      awk '/^[[:space:]]*Location:/{ print $2 }' | tail -n 1) || url=""
  else
    die "neither curl nor wget is installed"
  fi

  case "$url" in
    */tag/*) printf '%s' "${url##*/tag/}" ;;
    *) die "cannot work out the newest version; set PMPX_VERSION" ;;
  esac
}

sha256_of() { # <file>
  if have sha256sum; then
    sha256sum "$1" | awk '{ print $1 }'
  elif have shasum; then
    shasum -a 256 "$1" | awk '{ print $1 }'
  elif have openssl; then
    openssl dgst -sha256 "$1" | awk '{ print $NF }'
  else
    die "no sha256 tool found (need one of sha256sum, shasum, openssl)"
  fi
}

# The hash listed for one file. GNU sha256sum writes `hash  name` in text mode and
# `hash *name` in binary mode, so both spellings are accepted -- the same rule the Rust side
# follows when it reads the same file.
expected_hash() { # <sums file> <file name>
  awk -v want="$2" '
    {
      name = $2
      sub(/^\*/, "", name)
      if (name == want) { print $1; exit }
    }
  ' "$1"
}

# ---------------------------------------------------------------------------

main() {
  target=$(target)

  version="${PMPX_VERSION:-$(latest_version)}"
  case "$version" in
    v*) ;;
    *) version="v$version" ;;
  esac

  base="${PMPX_BASE_URL:-https://github.com/$REPO/releases/download}"
  archive="$BIN-$target.tar.gz"
  dir="${PMPX_INSTALL_DIR:-$HOME/.local/bin}"

  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT INT TERM

  note "Downloading $base/$version/$archive"
  download "$base/$version/$archive" "$tmp/$archive"
  download "$base/$version/SHA256SUMS" "$tmp/SHA256SUMS"

  expected=$(expected_hash "$tmp/SHA256SUMS" "$archive")
  [ -n "$expected" ] || die "SHA256SUMS does not list $archive"

  actual=$(sha256_of "$tmp/$archive")
  if [ "$expected" != "$actual" ]; then
    die "checksum mismatch for $archive
  expected $expected
  got      $actual
Nothing was installed."
  fi

  tar -xzf "$tmp/$archive" -C "$tmp"
  [ -f "$tmp/$BIN" ] || die "$archive does not contain $BIN"
  chmod 755 "$tmp/$BIN"

  # Everything that can be checked is checked *before* anything lands in the install
  # directory: a mirror serving a stale archive under a newer tag should leave nothing
  # behind, not a working install of the wrong version.
  downloaded=$("$tmp/$BIN" --version) || die "the downloaded $BIN does not run"
  reported=${downloaded##* }
  if [ "$reported" != "${version#v}" ]; then
    die "$version contains $reported; nothing was installed"
  fi

  mkdir -p "$dir"
  cp "$tmp/$BIN" "$dir/$BIN"
  chmod 755 "$dir/$BIN"

  installed=$("$dir/$BIN" --version) || die "$dir/$BIN was installed but does not run"
  note "$installed installed to $dir/$BIN"

  case ":${PATH:-}:" in
    *":$dir:"*) ;;
    *)
      note ""
      note "$dir is not on your PATH. Add it, for example:"
      note "  export PATH=\"$dir:\$PATH\""
      ;;
  esac

  # Two pmpx on PATH is a confusing thing to debug, and the one that runs is whichever
  # comes first -- which is probably not the one just installed.
  other=$(command -v "$BIN" 2>/dev/null || true)
  if [ -n "$other" ] && [ "$other" != "$dir/$BIN" ]; then
    note ""
    note "Note: another pmpx is on your PATH at $other; that one runs first."
  fi
}

main "$@"
