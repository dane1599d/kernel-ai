#!/bin/bash
# Signs a Kernel AI release's TUF roles with the founder's two key files
# (P4 task 18, interim: docs/release/root-ceremony.md, "Interim: key files").
#
#   env -i /bin/bash --noprofile --norc release-sign.sh KEYS WORK
#
# KEYS holds root.key and release.key, each encrypted with its own passphrase.
# Each <role>.canon in WORK (from `cargo xtask release-repo`) with no .sig is
# copied once to a private folder, and that copy is hashed and signed: root
# with root.key, every other role with release.key. The .sig is the raw Ed25519
# signature in hex. Each passphrase is asked once, never echoed, and passed to
# openssl on a pipe. No network call; only new .sig files are written. It runs
# in bash 3.2 (macOS) and later, with OpenSSL 3 at a fixed path.
set -euo pipefail -o noclobber
umask 077
ulimit -c 0
export PATH=/usr/bin:/bin LC_ALL=C

[ $# -eq 2 ] || { echo "usage: release-sign.sh KEYS WORK" >&2; exit 2; }
keys=$1 work=$2
openssl=
for at in /usr/bin/openssl /opt/homebrew/opt/openssl@3/bin/openssl /usr/local/opt/openssl@3/bin/openssl; do
  case "$("$at" version 2>/dev/null || true)" in
    "OpenSSL 3."*) openssl=$at; break ;;
  esac
done
[ -n "$openssl" ] || { echo "no OpenSSL 3 at /usr/bin or Homebrew's openssl@3" >&2; exit 1; }
echo "using $openssl: $("$openssl" version)" >&2
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

root=() release=() todo=0
for canon in "$work"/*.canon; do
  [ -e "$canon" ] || [ -L "$canon" ] || continue
  sig=${canon%.canon}.sig
  if [ -L "$canon" ] || [ ! -f "$canon" ] || [ -L "$sig" ]; then
    echo "$canon or its .sig is not a plain file: nothing signed" >&2; exit 1
  fi
  [ -e "$sig" ] && continue
  case "$(basename "$canon" .canon)" in
    root) root+=("$canon"); todo=$((todo + 1)) ;;
    targets|packages|models|snapshot|timestamp) release+=("$canon"); todo=$((todo + 1)) ;;
    *) echo "unexpected $canon: nothing signed" >&2; exit 1 ;;
  esac
done

# sign KEY FILE...: asks for KEY's passphrase, checks it, then signs each FILE.
sign() {
  local key="$keys/$1.key" pass sig canon
  shift
  [ $# -gt 0 ] || return 0
  IFS= read -rsp "Passphrase for $(basename "$key"): " pass
  echo >&2
  printf '%s\n' "$pass" | "$openssl" pkey -in "$key" -passin stdin -noout ||
    { echo "not the passphrase of $key: nothing signed with it" >&2; exit 1; }
  for canon in "$@"; do
    cp -- "$canon" "$tmp/message"
    sig=$(printf '%s\n' "$pass" |
      "$openssl" pkeyutl -sign -rawin -inkey "$key" -passin stdin -in "$tmp/message" |
      od -An -v -tx1 | tr -d ' \n')
    [ ${#sig} -eq 128 ] || { echo "no Ed25519 signature for $canon" >&2; exit 1; }
    printf '%s\n' "$sig" > "${canon%.canon}.sig"
    echo "signed $(basename "$canon") with $(basename "$key"): sha256 $("$openssl" dgst -sha256 -r "$tmp/message" | cut -d' ' -f1)"
    rm -f -- "$tmp/message"
  done
  pass=
}

# ${a[@]+"${a[@]}"}: an empty array under `set -u`, which bash 3.2 refuses.
sign root ${root[@]+"${root[@]}"}
sign release ${release[@]+"${release[@]}"}
[ "$todo" -gt 0 ] || echo "nothing to sign in $work"
