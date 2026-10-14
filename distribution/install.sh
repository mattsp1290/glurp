#!/bin/sh
# Install the glurp release binary into $GLURP_INSTALL_DIR (default $HOME/.local/bin).
#
# Environment:
#   GLURP_INSTALL_DIR        destination directory
#   GLURP_INSTALL_VERSION    release tag such as v0.3.0 (default: latest)
#   GLURP_RELEASE_BASE_URL   release base URL, https:// or file://
set -eu

default_base="https://github.com/mattsp1290/glurp/releases"
tmp=

fail() {
  printf 'install.sh: %s\n' "$1" >&2
  exit 1
}

cleanup() {
  if [ -n "$tmp" ]; then
    rm -f "$tmp"
  fi
}

valid_tag() {
  case $1 in
    '' | *[!0-9.v]*) return 1 ;;
  esac
  # No leading zeros: one spelling per version.
  printf '%s\n' "$1" |
    awk '/^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/ { ok = 1 } END { exit !ok }'
}

# fetch <max-seconds> <url> <output|->
# -q comes first so ~/.curlrc cannot change the output, --globoff keeps {} and
# [] in a URL literal, and the time bound covers every retry.
fetch() {
  max=$1 url=$2 out=$3
  set -- -q -fsSL --globoff --connect-timeout 10 --retry 3 --retry-max-time "$max" --max-time "$max"
  if [ "$secure" = 1 ]; then
    set -- "$@" --proto '=https' --proto-redir '=https'
  fi
  curl "$@" --url "$url" -o "$out"
}

# field <block> <key>: print a value from the manifest in $manifest. An empty
# block selects top-level keys; otherwise the key is read from that object.
field() {
  printf '%s\n' "$manifest" | awk -v blk="$1" -v key="$2" '
    blk != "" && $0 == "  \"" blk "\": {" { inb = 1; next }
    inb && /^  },?$/ { inb = 0; next }
    {
      if (blk != "") {
        if (!inb) next
        prefix = "    \"" key "\": "
      } else {
        prefix = "  \"" key "\": "
      }
      if (index($0, prefix) != 1) next
      v = substr($0, length(prefix) + 1)
      sub(/,$/, "", v)
      gsub(/"/, "", v)
      print v
      exit
    }'
}

sha256_of() {
  case $hasher in
    sha256sum) sha256sum | awk '{ print $1 }' ;;
    shasum) shasum -a 256 | awk '{ print $1 }' ;;
    openssl) openssl dgst -sha256 | awk '{ print $NF }' ;;
  esac
}

main() {
  case $(uname -s) in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    *) fail "unsupported OS: $(uname -s)" ;;
  esac
  case $(uname -m) in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) fail "unsupported architecture: $(uname -m)" ;;
  esac
  target="$os-$arch"

  command -v curl >/dev/null 2>&1 || fail "curl is required"
  command -v awk >/dev/null 2>&1 || fail "awk is required"
  if command -v sha256sum >/dev/null 2>&1; then
    hasher=sha256sum
  elif command -v shasum >/dev/null 2>&1; then
    hasher=shasum
  elif command -v openssl >/dev/null 2>&1; then
    hasher=openssl
  else
    fail "sha256sum, shasum or openssl is required"
  fi

  want=${GLURP_INSTALL_VERSION:-}
  if [ -n "$want" ] && ! valid_tag "$want"; then
    fail "GLURP_INSTALL_VERSION must look like v1.2.3"
  fi
  base=${GLURP_RELEASE_BASE_URL:-$default_base}
  while :; do
    case $base in
      */) base=${base%/} ;;
      *) break ;;
    esac
  done
  case $base in
    https://*) secure=1 ;;
    file://*) secure=0 ;;
    *) fail "GLURP_RELEASE_BASE_URL must start with https:// or file://" ;;
  esac

  if [ -n "$want" ]; then
    manifest_url="$base/download/$want/glurp-manifest.json"
  else
    manifest_url="$base/latest/download/glurp-manifest.json"
  fi
  manifest=$(fetch 20 "$manifest_url" -) || fail "could not download $manifest_url"

  [ "$(field '' schema)" = 1 ] || fail "unsupported manifest schema"
  version=$(field '' version)
  valid_tag "v$version" || fail "manifest has an invalid version"
  [ "$(field '' tag)" = "v$version" ] || fail "manifest tag does not match its version"
  if [ -n "$want" ] && [ "v$version" != "$want" ]; then
    fail "manifest version v$version does not match requested $want"
  fi
  url=$(field assets "$target")
  digest=$(field sha256 "$target")
  [ -n "$url" ] || fail "target $target not found in manifest"
  [ "$url" = "$base/download/v$version/glurp-$target" ] || fail "unexpected asset URL for $target"
  # Ranges such as a-f can also match uppercase letters in some locales.
  case $digest in
    '' | *[!0123456789abcdef]*) fail "invalid sha256 for $target" ;;
  esac
  [ "${#digest}" -eq 64 ] || fail "invalid sha256 for $target"

  dir=${GLURP_INSTALL_DIR:-${HOME:?HOME is not set}/.local/bin}
  # Absolute and without trailing slashes, so it compares equal to PATH
  # entries and can never be read as an option.
  case $dir in
    /*) ;;
    *) dir=$PWD/$dir ;;
  esac
  while :; do
    case $dir in
      ?*/) dir=${dir%/} ;;
      *) break ;;
    esac
  done
  mkdir -p "$dir" || fail "could not create $dir"
  # mv would otherwise move the download into the directory and report success.
  [ ! -d "$dir/glurp" ] || fail "$dir/glurp is a directory"
  trap cleanup EXIT
  trap 'cleanup; exit 1' INT TERM HUP
  tmp=$(mktemp "$dir/.glurp-install.XXXXXX") || fail "could not create a temporary file in $dir"

  fetch 300 "$url" "$tmp" || fail "could not download $url"
  actual=$(sha256_of <"$tmp")
  [ "$actual" = "$digest" ] || fail "checksum mismatch for $target"

  chmod 0755 "$tmp"
  # Bounded where coreutils timeout exists; macOS ships none.
  if command -v timeout >/dev/null 2>&1; then
    reported=$(timeout 10 "$tmp" --version 2>/dev/null) ||
      fail "downloaded binary does not run on this system"
  else
    reported=$("$tmp" --version 2>/dev/null) || fail "downloaded binary does not run on this system"
  fi
  [ "$reported" = "glurp v$version" ] || fail "downloaded binary reports '$reported', expected 'glurp v$version'"

  mv -f "$tmp" "$dir/glurp"
  tmp=

  if [ -t 1 ]; then
    printf '\033[32mInstalled\033[0m glurp v%s to %s/glurp\n' "$version" "$dir"
  else
    printf 'Installed glurp v%s to %s/glurp\n' "$version" "$dir"
  fi

  case ":$PATH:" in
    *":$dir:"*) ;;
    *)
      printf 'warning: %s is not on your PATH. Add it with:\n' "$dir" >&2
      # shellcheck disable=SC2016 # $PATH is printed for the user to paste
      printf '  export PATH="%s:$PATH"\n' "$dir" >&2
      ;;
  esac
  command -v ssh >/dev/null 2>&1 ||
    printf 'warning: ssh was not found on PATH; glurp needs OpenSSH at runtime\n' >&2
  found=$(command -v glurp 2>/dev/null || true)
  if [ -n "$found" ] && [ "$found" != "$dir/glurp" ]; then
    printf 'warning: another glurp shadows this install: %s\n' "$found" >&2
  fi
}

main "$@"
