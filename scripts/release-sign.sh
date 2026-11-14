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
# each model and package it signs and when each file expires, refusing any
# file that is not the text xtask writes. It then signs each copy,
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
# (and root, at the first release only), then timestamp alone. Every role but
# root is at the snapshot's version, this release's number, signed once.
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
rk=$(hex "$keys/root.pub") rel=$(hex "$keys/release.pub")
echo "root.key is $rk"
echo "release.key is $rel"
count() { (grep -o -- "$2" "$1" || true) | wc -l | tr -d ' '; }
for copy in "$@"; do
  name=$(basename "$copy")
  [ "$(count "$copy" '"public"')" = "$(count "$copy" '"keyval":{"public":"[0-9a-f]\{64\}"}')" ] &&
    [ "$(count "$copy" '"keyval"')" = "$(count "$copy" '"keyval":{"public"')" ] &&
    [ "$(count "$copy" '"keytype"')" = "$(count "$copy" '"keytype":"ed25519","keyval"')" ] &&
    [ "$(count "$copy" '"scheme"')" = "$(count "$copy" '"scheme":"ed25519"}')" ] ||
    { echo "$name.canon names a key that is not lower-case hex Ed25519: nothing signed" >&2; exit 1; }
  echo "$name.canon names: $( (grep -o '"public":"[0-9a-f]*"' "$copy" || true) | cut -d'"' -f4 | tr '\n' ' ')"
done
# A key is named by its id, the SHA-256 of its canonical form, and the two
# keys are two.
id() { printf '{"keytype":"ed25519","keyval":{"public":"%s"},"scheme":"ed25519"}' "$1" | "$openssl" dgst -sha256 -r | cut -c1-64; }
rid=$(id "$rk") lid=$(id "$rel")
[ "$rid" != "$lid" ] || { echo "root.key and release.key are one key: nothing signed" >&2; exit 1; }
key() { printf '"%s":{"keytype":"ed25519","keyval":{"public":"%s"},"scheme":"ed25519"}' "$1" "$2"; }
of() { printf '"%s":{"keyids":["%s"],"threshold":1}' "$1" "$2"; }
if [[ $rid < $lid ]]; then ks=$(key "$rid" "$rk"),$(key "$lid" "$rel"); else ks=$(key "$lid" "$rel"),$(key "$rid" "$rk"); fi
# In round 2 the timestamp must name the snapshot round 1 signed: its canon and
# signature here, checked with release.key, hashed as the device fetches it.
snap= size=
if [ "$round" = 2 ]; then
  cp -- "$work/snapshot.canon" "$tmp/.snapshot" && cp -- "$work/snapshot.sig" "$tmp/.sig" || exit 1
  sig=$(cat "$tmp/.sig")
  case "$sig" in *[!0-9a-f]*) sig= ;; esac
  [ ${#sig} -eq 128 ] && printf "$(printf '%s' "$sig" | sed 's/../\\x&/g')" > "$tmp/.raw" &&
    "$openssl" pkeyutl -verify -pubin -inkey "$keys/release.pub" -rawin -in "$tmp/.snapshot" -sigfile "$tmp/.raw" >/dev/null 2>&1 ||
    { echo "snapshot.sig is not release.key's signature of snapshot.canon: nothing signed" >&2; exit 1; }
  signed() { printf '{"signatures":[{"keyid":"%s","sig":"%s"}],"signed":' "$lid" "$sig"; cat "$tmp/.snapshot"; printf '}'; }
  snap=$(signed | "$openssl" dgst -sha256 -r | cut -c1-64) size=$(signed | wc -c | tr -d ' ')
  echo "round 1 signed snapshot.canon as $size bytes, sha256 $snap: timestamp.canon must name it"
fi
# root (version 1, root.key for root and release.key for the rest), targets
# (no target of its own; *.kpkg to packages, then *.gguf and *.chunks to
# models, both release.key's), snapshot and timestamp are fixed texts, their
# dates and the spec version aside: no other role can speak for a package or
# a model, and each _type is the file's own.
stamp() { sed -e 's/"expires":"[0-9:TZ-]*"/"expires":"E"/' -e 's/"spec_version":"[0-9.]*"/"spec_version":"S"/' "$1"; }
norm() { stamp "$1" | sed "s/\"version\":$seq\([,}]\)/\"version\":N\1/g"; }
ends='"terminating":true,"threshold":1}' v='{"version":N}'
rw='{"_type":"root","consistent_snapshot":true,"expires":"E","keys":{'$ks'},"roles":{'$(of root "$rid")','
rw=$rw$(of snapshot "$lid")','$(of targets "$lid")','$(of timestamp "$lid")'},"spec_version":"S","version":1}'
tw='{"_type":"targets","delegations":{"keys":{'$(key "$lid" "$rel")'},"roles":[{"keyids":["'$lid'"],"name":"packages",'
tw=$tw'"paths":["*.kpkg"],'$ends',{"keyids":["'$lid'"],"name":"models","paths":["*.gguf","*.chunks"],'$ends']},'
tw=$tw'"expires":"E","spec_version":"S","targets":{},"version":N}'
sw='{"_type":"snapshot","expires":"E","meta":{"models.json":'$v',"packages.json":'$v',"targets.json":'$v'},'
sw=$sw'"spec_version":"S","version":N}'
mw='{"_type":"timestamp","expires":"E","meta":{"snapshot.json":{"hashes":{"sha256":"'$snap'"},"length":'$size','
mw=$mw'"version":N}},"spec_version":"S","version":N}'
pin() {
  [ ! -e "$tmp/$1" ] || [ "$($3 "$tmp/$1")" = "$2" ] ||
    { echo "$1.canon is not what xtask writes: nothing signed" >&2; exit 1; }
}
pin root "$rw" stamp
pin targets "$tw" norm
pin snapshot "$sw" norm
pin timestamp "$mw" norm
# A delegated role's targets, walked as JSON (awk -v role=models|packages): the
# role must be exactly the shape xtask writes, each model's path an id the OS
# takes and .gguf, or a lower-case SHA-256 and .chunks, and each entry its
# length, SHA-256 and, but for a chunk list, its index. Each line printed must
# be on the prepare run's page on GitHub.
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
    function field(e, k,   f) {
      if (!(f = index(e, "\"" k "\":"))) return "none"
      f += length(k) + 3
      return substr(e, f, value(e, f) - f)
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
        line = role ".canon signs " key ", sha256 " substr(rest, 23, 64)
        if (kpkg) line = line ", grants " field(e, "grants") ", actions " field(e, "actions") ", yanked " field(e, "yanked")
        else line = line ", length " substr(rest, 99, length(rest) - 99)
        if (gguf) line = line ", name " field(e, "name") ", licence " field(e, "licence") ", source " field(e, "source")
        if (line !~ /^[ -~]+$/) exit 1
        print line
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
# Each role expires its full period from now, less up to the week signing may
# take: root, targets, packages and models 365 days, snapshot 30 and timestamp
# 7. This machine's clock must be right.
now=$(date -u +%s)
for copy in "$@"; do
  name=$(basename "$copy")
  case "$name" in snapshot) days=30 ;; timestamp) days=7 ;; *) days=365 ;; esac
  awk -v days="$days" -v now="$now" -v name="$name" '
    function day(y, m, d) {
      y += 0; m += 0; y -= (m <= 2); m += (m > 2) ? -3 : 9
      return 365 * y + int(y / 4) - int(y / 100) + int(y / 400) + int((153 * m + 2) / 5) + d - 719469
    }
    { s = s $0 }
    END {
      if (!match(s, /"expires":"[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9]Z"/)) {
        print name ".canon has no expiry: nothing signed" > "/dev/stderr"
        exit 1
      }
      t = substr(s, RSTART + 11, 20)
      at = day(substr(t, 1, 4), substr(t, 6, 2), substr(t, 9, 2)) * 86400
      at += substr(t, 12, 2) * 3600 + substr(t, 15, 2) * 60 + substr(t, 18, 2)
      line = name ".canon expires " t ", in " int((at - now) / 86400 + 0.5) " days"
      if (at >= now + (days - 7) * 86400 && at <= now + (days + 1) * 86400) { print line; exit }
      print line ", not within a week of " days ": nothing signed" > "/dev/stderr"
      exit 1
    }' "$copy" || exit 1
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
