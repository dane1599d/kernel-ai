#!/bin/bash
# Signs a Kernel AI release's TUF roles with the founder's two key files
# (P4 task 18, interim: docs/release/root-ceremony.md, "Interim: key files").
#
#   env -i /bin/bash --noprofile --norc release-sign.sh KEYS WORK
#
# KEYS holds root.key and release.key, each encrypted with its own passphrase,
# and their root.pub and release.pub. Each <role>.canon in WORK with no .sig is
# copied once to a private folder; before any passphrase, it prints the two
# keys and the keys each copy names. It then signs each copy, root with
# root.key and the rest with release.key, and writes the raw Ed25519 signature
# in hex as <role>.sig. Passphrases are never echoed and reach openssl on a
# pipe. No network call. Bash 3.2 (macOS) and later; OpenSSL 3 at a fixed path.
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
echo "using $openssl: $("$openssl" version)"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

root=() release=()
for canon in "$work"/*.canon; do
  [ -e "$canon" ] || [ -L "$canon" ] || continue
  name=$(basename "$canon" .canon)
  if [ -L "$canon" ] || [ ! -f "$canon" ] || [ -L "$work/$name.sig" ]; then
    echo "$canon or its .sig is not a plain file: nothing signed" >&2; exit 1
  fi
  [ -e "$work/$name.sig" ] && continue
  case "$name" in
    root) root+=("$name") ;;
    targets|packages|models|snapshot|timestamp) release+=("$name") ;;
    *) echo "unexpected $canon: nothing signed" >&2; exit 1 ;;
  esac
  cp -- "$canon" "$tmp/$name"
done
set -- "$tmp"/*
[ -e "$1" ] || { echo "nothing to sign in $work"; exit 0; }

# Every number below must be one of the two on your paper.
hex() { "$openssl" pkey -pubin -in "$1" -outform DER | tail -c 32 | od -An -v -tx1 | tr -d ' \n'; }
echo "root.key is $(hex "$keys/root.pub")"
echo "release.key is $(hex "$keys/release.pub")"
for copy in "$@"; do
  echo "$(basename "$copy").canon names: $( (grep -o '"public":"[0-9a-f]*"' "$copy" || true) | cut -d'"' -f4 | sort -u | tr '\n' ' ')"
done

# sign KEY ROLE...: asks for KEY's passphrase, checks it, then signs each ROLE.
sign() {
  local key="$keys/$1.key" pass sig name
  shift
  [ $# -gt 0 ] || return 0
  IFS= read -rsp "Passphrase for $(basename "$key"): " pass
  echo >&2
  printf '%s\n' "$pass" | "$openssl" pkey -in "$key" -passin stdin -noout ||
    { echo "not the passphrase of $key: nothing signed with it" >&2; exit 1; }
  for name in "$@"; do
    sig=$(printf '%s\n' "$pass" | "$openssl" pkeyutl -sign -rawin -inkey "$key" -passin stdin -in "$tmp/$name" |
      od -An -v -tx1 | tr -d ' \n')
    [ ${#sig} -eq 128 ] || { echo "no Ed25519 signature for $name.canon" >&2; exit 1; }
    printf '%s\n' "$sig" > "$work/$name.sig"
    echo "signed $name.canon with $(basename "$key"): sha256 $("$openssl" dgst -sha256 -r "$tmp/$name" | cut -d' ' -f1)"
  done
  pass=
}

# ${a[@]+"${a[@]}"}: an empty array under `set -u`, which bash 3.2 refuses.
sign root ${root[@]+"${root[@]}"}
sign release ${release[@]+"${release[@]}"}
