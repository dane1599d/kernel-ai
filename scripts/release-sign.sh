#!/bin/bash
# Signs a Kernel AI release's TUF roles with the founder's two key files
# (P4 task 18, interim: docs/release/root-ceremony.md, "Interim: key files").
#
#   env -i /bin/bash --noprofile --norc release-sign.sh KEYS WORK
#
# KEYS holds root.key and release.key, each encrypted with its own passphrase,
# and their root.pub and release.pub. Each <role>.canon in WORK with no .sig is
# copied once to a private folder, and they must be one round's files. Before
# any passphrase, it prints the round, the two keys, the keys each copy names,
# and each model and package it signs, refusing any file that is not the text
# xtask writes. It then signs each copy,
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
# Each round signs exactly its files: targets, packages, models and snapshot
# (and root, at the first release only), then timestamp alone. Every role is
# at the snapshot's version, this release's number.
files=$(cd "$tmp" && echo *)
case "$files" in
  "models packages snapshot targets" | "models packages root snapshot targets") round=1 at=$tmp/snapshot ;;
  timestamp) round=2 at=$work/snapshot.canon ;;
  *) echo "$work holds $files to sign, not one round's files: nothing signed" >&2; exit 1 ;;
esac
seq=$(sed -n 's/^{"_type":"snapshot",.*,"version":\([1-9][0-9]*\)}$/\1/p' "$at" 2>/dev/null || true)
[ -n "$seq" ] || { echo "$at is not a snapshot: nothing signed" >&2; exit 1; }
echo "round $round of release $seq: signs $files"

# Every number below must be one of the two on your paper. Each key a file
# names must be a plain Ed25519 key in lower-case hex, so every one is shown.
hex() { "$openssl" pkey -pubin -in "$1" -outform DER | tail -c 32 | od -An -v -tx1 | tr -d ' \n'; }
rel=$(hex "$keys/release.pub")
echo "root.key is $(hex "$keys/root.pub")"
echo "release.key is $rel"
count() { (grep -o -- "$2" "$1" || true) | wc -l | tr -d ' '; }
for copy in "$@"; do
  name=$(basename "$copy")
  [ "$(count "$copy" '"public"')" = "$(count "$copy" '"keyval":{"public":"[0-9a-f]\{64\}"}')" ] &&
    [ "$(count "$copy" '"keyval"')" = "$(count "$copy" '"keyval":{"public"')" ] &&
    [ "$(count "$copy" '"keytype"')" = "$(count "$copy" '"keytype":"ed25519","keyval"')" ] &&
    [ "$(count "$copy" '"scheme"')" = "$(count "$copy" '"scheme":"ed25519"}')" ] ||
    { echo "$name.canon names a key that is not lower-case hex Ed25519: nothing signed" >&2; exit 1; }
  echo "$name.canon names: $( (grep -o '"public":"[0-9a-f]*"' "$copy" || true) | cut -d'"' -f4 | sort -u | tr '\n' ' ')"
done
# targets.canon lists no target itself and delegates *.kpkg to packages, then
# *.gguf and *.chunks to models, with release.key alone: no other role can
# speak for a package or a model. With hashes, dates, the spec version and
# lengths aside, it, snapshot and timestamp are fixed texts, each its own _type.
norm() { sed -e 's/"[0-9a-f]\{64\}"/"H"/g' -e 's/"expires":"[0-9:TZ-]*"/"expires":"E"/' \
  -e 's/"spec_version":"[0-9.]*"/"spec_version":"S"/' -e "s/\"version\":$seq\([,}]\)/\"version\":N\1/g" \
  -e 's/"length":[0-9]*/"length":L/g' "$1"; }
ends='"terminating":true,"threshold":1}' v='{"version":N}'
tw='{"_type":"targets","delegations":{"keys":{"H":{"keytype":"ed25519","keyval":{"public":"H"},"scheme":"ed25519"}},'
tw=$tw'"roles":[{"keyids":["H"],"name":"packages","paths":["*.kpkg"],'$ends',{"keyids":["H"],"name":"models",'
tw=$tw'"paths":["*.gguf","*.chunks"],'$ends']},"expires":"E","spec_version":"S","targets":{},"version":N}'
sw='{"_type":"snapshot","expires":"E","meta":{"models.json":'$v',"packages.json":'$v',"targets.json":'$v'},'
sw=$sw'"spec_version":"S","version":N}'
mw='{"_type":"timestamp","expires":"E","meta":{"snapshot.json":{"hashes":{"sha256":"H"},"length":L,"version":N}},'
mw=$mw'"spec_version":"S","version":N}'
for pair in "targets:$tw" "snapshot:$sw" "timestamp:$mw"; do
  name=${pair%%:*}
  [ ! -e "$tmp/$name" ] || [ "$(norm "$tmp/$name")" = "${pair#*:}" ] ||
    { echo "$name.canon is not what xtask writes: nothing signed" >&2; exit 1; }
done
[ ! -e "$tmp/targets" ] || grep -qF "\"public\":\"$rel\"" "$tmp/targets" ||
  { echo "targets.canon delegates to a key that is not release.key: nothing signed" >&2; exit 1; }
# A delegated role's targets, walked as JSON (awk -v role=models|packages): the
# role must be exactly the shape xtask writes, each model's path an id the OS
# takes and .gguf, or a lower-case SHA-256 and .chunks, and each entry its
# length, SHA-256 and, but for a chunk list, its index. Each line printed, a
# package's with its grants, must be on the prepare run's page on GitHub.
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
        kpkg = role == "packages" && key ~ /^[ -~]+[.]kpkg$/
        gguf = role == "models" && key ~ /^[a-z0-9_-][a-z0-9._-]*[.]gguf$/ && length(key) <= 69
        list = role == "models" && key ~ /^[0-9a-f]+[.]chunks$/ && length(key) == 71
        if ((kpkg || gguf) && index(e, pre) == 1) rest = substr(e, value(e, length(pre) + 1))
        else if (list) rest = "}," substr(e, 2)
        else exit 1
        if (rest !~ /^[}],"hashes":[{]"sha256":"[0-9a-f]+"[}],"length":[0-9]+[}]$/ || substr(rest, 87, 1) != "\"") exit 1
        sha = substr(rest, 23, 64)
        if (!kpkg) { print "models.canon signs " key ", sha256 " sha ", length " substr(rest, 99, length(rest) - 99); continue }
        g = index(e, "\"grants\":[")
        grants = g ? substr(e, g + 9, value(e, g + 9) - g - 9) : "none"
        if (grants !~ /^[ -~]+$/) exit 1
        print "packages.canon signs " key ", sha256 " sha ", grants " grants
      }
      if (substr(s, i) != "},\"version\":" seq "}") exit 1
    }'
for role in models packages; do
  [ -e "$tmp/$role" ] || continue
  listed=$(awk -v role="$role" -v seq="$seq" "$walk" "$tmp/$role") ||
    { echo "$role.canon is not what xtask writes: nothing signed" >&2; exit 1; }
  n=$(($(count "$tmp/$role" '\.kpkg":{') + $(count "$tmp/$role" '\.gguf":{') + $(count "$tmp/$role" '\.chunks":{')))
  [ "$(printf '%s' "$listed" | grep -c "^$role" || true)" = "$n" ] ||
    { echo "$role.canon lists $n targets, not each once: nothing signed" >&2; exit 1; }
  [ -z "$listed" ] || printf '%s\n' "$listed"
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
