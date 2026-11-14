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
# openssl on a pipe. No network call; only new .sig files are written. Run the
# copy on the key stick whose SHA-256 you checked.
set -euo pipefail -o noclobber
umask 077
ulimit -c 0
export PATH=/usr/bin:/bin LC_ALL=C
openssl=/usr/bin/openssl

[ $# -eq 2 ] || { echo "usage: release-sign.sh KEYS WORK" >&2; exit 2; }
keys=$1 work=$2
case "$("$openssl" version)" in
  "OpenSSL 3."*) ;;
  *) echo "$openssl is not OpenSSL 3" >&2; exit 1 ;;
esac
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

root=() release=()
for canon in "$work"/*.canon; do
  [ -e "$canon" ] || [ -L "$canon" ] || continue
  sig=${canon%.canon}.sig
  if [ -L "$canon" ] || [ ! -f "$canon" ] || [ -L "$sig" ]; then
    echo "$canon or its .sig is not a plain file: nothing signed" >&2; exit 1
  fi
  [ -e "$sig" ] && continue
  case "$(basename "$canon" .canon)" in
    root) root+=("$canon") ;;
    targets|packages|models|snapshot|timestamp) release+=("$canon") ;;
    *) echo "unexpected $canon: nothing signed" >&2; exit 1 ;;
  esac
done

# sign KEY FILE...: asks for KEY's passphrase, checks it, then signs each FILE.
sign() {
  local key="$keys/$1.key" pass sig
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
    echo "signed $(basename "$canon") with $(basename "$key"): sha256 $(sha256sum "$tmp/message" | cut -d' ' -f1)"
    rm -f -- "$tmp/message"
  done
  pass=
}

sign root "${root[@]}"
sign release "${release[@]}"
[ $((${#root[@]} + ${#release[@]})) -gt 0 ] || echo "nothing to sign in $work"
