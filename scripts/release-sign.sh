#!/bin/bash
# Signs a Kernel AI release's TUF roles with the founder's two key files
# (P4 task 18, interim: docs/release/root-ceremony.md, "Interim: key files").
#
#   env -i /bin/bash --noprofile --norc release-sign.sh KEYS WORK
#
# KEYS holds root.key and release.key, each encrypted with its own passphrase,
# and their root.pub and release.pub. Each <role>.canon in WORK with no .sig is
# copied once to a private folder; before any passphrase, it prints the two
# keys, the keys each copy names and each package it signs, with its grants,
# refusing a targets, models or packages file not shaped as xtask writes it.
# It then signs each copy,
# root with root.key and the rest with release.key, and writes the raw
# Ed25519 signature in hex as <role>.sig. Passphrases are never echoed and
# reach openssl on a pipe. No network call. Bash 3.2 (macOS) and later;
# OpenSSL 3 at a fixed path.
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
  # xtask writes one line of printable ASCII, with no newline: nothing hides.
  [ "$(tr -d ' -~' < "$tmp/$name" | wc -c | tr -d ' ')" = 0 ] ||
    { echo "$canon is not one line of printable ASCII: nothing signed" >&2; exit 1; }
done
set -- "$tmp"/*
[ -e "$1" ] || { echo "nothing to sign in $work"; exit 0; }

# Every number below must be one of the two on your paper.
hex() { "$openssl" pkey -pubin -in "$1" -outform DER | tail -c 32 | od -An -v -tx1 | tr -d ' \n'; }
rel=$(hex "$keys/release.pub")
echo "root.key is $(hex "$keys/root.pub")"
echo "release.key is $rel"
for copy in "$@"; do
  echo "$(basename "$copy").canon names: $( (grep -o '"public":"[0-9a-f]*"' "$copy" || true) | cut -d'"' -f4 | sort -u | tr '\n' ' ')"
done
# targets.canon lists no target itself and delegates *.kpkg to packages, then
# *.gguf and *.chunks to models, with release.key alone: no other role can
# speak for a package or a model. Hashes, dates and versions aside, it is fixed.
shape() { sed -e 's/"[0-9a-f]\{64\}"/"H"/g' -e 's/"expires":"[0-9:TZ-]*"/"expires":"E"/' \
  -e 's/"spec_version":"[0-9.]*"/"spec_version":"S"/' -e 's/"version":[0-9]*}$/"version":N}/' "$1"; }
ends='"terminating":true,"threshold":1}'
want='{"_type":"targets","delegations":{"keys":{"H":{"keytype":"ed25519","keyval":{"public":"H"},"scheme":"ed25519"}},'
want=$want'"roles":[{"keyids":["H"],"name":"packages","paths":["*.kpkg"],'$ends',{"keyids":["H"],"name":"models",'
want=$want'"paths":["*.gguf","*.chunks"],'$ends']},"expires":"E","spec_version":"S","targets":{},"version":N}'
if [ -e "$tmp/targets" ] && { [ "$(shape "$tmp/targets")" != "$want" ] || ! grep -qF "\"public\":\"$rel\"" "$tmp/targets"; }; then
  echo "targets.canon is not what xtask writes: nothing signed" >&2; exit 1
fi
# A delegated role's targets, walked as JSON (awk -v role=models|packages): the
# role must be exactly the shape xtask writes, each model's path a .gguf or a
# .chunks, and each package's entry its length, SHA-256 and index, printed
# with its grants. Each must be in the prepare run's list on GitHub.
walk='
    function value(t, i,   c, d, q) {
      c = substr(t, i, 1)
      if (c != "\"" && c != "{" && c != "[") { while (substr(t, i, 1) ~ /[-0-9a-z.]/) i++; return i }
      for (d = q = 0; i <= length(t); i++) {
        c = substr(t, i, 1)
        if (q) { if (c == "\\") i++; else if (c == "\"") q = 0 }
        else if (c == "\"") q = 1
        else if (c == "{" || c == "[") d++
        else if (c == "}" || c == "]") d--
        if (!q && !d) return i + 1
      }
      exit 1
    }
    { s = s $0 }
    END {
      top = "^[{]\"_type\":\"targets\",\"expires\":\"[0-9:TZ-]+\",\"spec_version\":\"[0-9.]+\",\"targets\":[{]"
      pre = "{\"custom\":{\"kai\":"
      if (NR != 1 || !match(s, top)) exit 1
      for (i = RLENGTH + 1; substr(s, i, 1) != "}"; i = k) {
        if (n++ && substr(s, i++, 1) != ",") exit 1
        j = value(s, i); key = substr(s, i + 1, j - i - 2); k = value(s, j + 1); e = substr(s, j + 1, k - j - 1)
        if (substr(s, j, 1) != ":") exit 1
        if (role == "models") { if (key !~ /^[ -~]+[.](gguf|chunks)$/) exit 1; continue }
        if (key !~ /^[ -~]+[.]kpkg$/ || index(e, pre) != 1) exit 1
        rest = substr(e, value(e, length(pre) + 1))
        if (rest !~ /^[}],"hashes":[{]"sha256":"[0-9a-f]+"[}],"length":[0-9]+[}]$/ || substr(rest, 87, 1) != "\"") exit 1
        g = index(e, "\"grants\":[")
        grants = g ? substr(e, g + 9, value(e, g + 9) - g - 9) : "none"
        if (grants !~ /^[ -~]+$/) exit 1
        print "packages.canon signs " key ", sha256 " substr(rest, 23, 64) ", grants " grants
      }
      if (substr(s, i) !~ /^[}],"version":[0-9]+[}]$/) exit 1
    }'
if [ -e "$tmp/models" ]; then
  awk -v role=models "$walk" "$tmp/models" || { echo "models.canon is not what xtask writes: nothing signed" >&2; exit 1; }
fi
if [ -e "$tmp/packages" ]; then
  listed=$(awk -v role=packages "$walk" "$tmp/packages") ||
    { echo "packages.canon is not what xtask writes: nothing signed" >&2; exit 1; }
  count=$( (grep -o '\.kpkg":{' "$tmp/packages" || true) | wc -l | tr -d ' ')
  [ "$(printf '%s' "$listed" | grep -c '^packages' || true)" = "$count" ] ||
    { echo "packages.canon lists $count packages, not each once: nothing signed" >&2; exit 1; }
  [ -z "$listed" ] || printf '%s\n' "$listed"
fi

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
