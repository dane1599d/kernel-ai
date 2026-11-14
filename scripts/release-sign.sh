#!/bin/bash
# Signs a Kernel AI release's TUF roles with the founder's two key files
# (P4 task 18, interim: docs/release/root-ceremony.md, "Interim: key files").
#
#   env -i /bin/bash --noprofile --norc release-sign.sh KEYS WORK
#
# KEYS holds root.key and release.key: Ed25519 private keys, each encrypted
# with its own passphrase. WORK holds the <role>.canon files that
# `cargo xtask release-repo prepare` or `prepare-timestamp` wrote. Each one
# without a <role>.sig is signed, root.canon with root.key and every other with
# release.key, and its .sig is the raw Ed25519 signature in hex, which
# `cargo xtask release-repo` checks. Each key's passphrase is asked once,
# never echoed, and passed to openssl on a pipe.
#
# It makes no network call, writes only .sig files, and runs /usr/bin/openssl
# by its full path. Run the copy on the key stick whose SHA-256 you checked.
set -euo pipefail
umask 077
export PATH=/usr/bin:/bin LC_ALL=C
openssl=/usr/bin/openssl

[ $# -eq 2 ] || { echo "usage: release-sign.sh KEYS WORK" >&2; exit 2; }
keys=$1 work=$2
case "$("$openssl" version)" in
  "OpenSSL 3."*) ;;
  *) echo "$openssl is not OpenSSL 3" >&2; exit 1 ;;
esac

root=() release=()
for canon in "$work"/*.canon; do
  [ -e "$canon" ] || continue
  [ -e "${canon%.canon}.sig" ] && continue
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
    sig=$(printf '%s\n' "$pass" |
      "$openssl" pkeyutl -sign -rawin -inkey "$key" -passin stdin -in "$canon" |
      od -An -v -tx1 | tr -d ' \n')
    [ ${#sig} -eq 128 ] || { echo "no Ed25519 signature for $canon" >&2; exit 1; }
    printf '%s\n' "$sig" > "${canon%.canon}.sig"
    echo "signed $(basename "$canon") ($(sha256sum "$canon" | cut -c1-16)) with $(basename "$key")"
  done
  pass=
}

sign root "${root[@]}"
sign release "${release[@]}"
[ $((${#root[@]} + ${#release[@]})) -gt 0 ] || echo "nothing to sign in $work"
