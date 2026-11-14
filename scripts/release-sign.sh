#!/bin/bash
# Signs a Kernel AI release's TUF roles with the founder's two key files
# (P4 task 18, interim: docs/release/root-ceremony.md, "Interim: key files").
#
#   env -i /bin/bash --noprofile --norc release-sign.sh KEYS WORK
#
# KEYS holds root.key and release.key, each encrypted with its own passphrase,
# and their root.pub and release.pub, and the ledger signed.log, a line for
# each round they signed. Each <role>.canon in WORK with no .sig is copied once
# to a private folder, and they must be one round's files. Before any
# passphrase, it prints the clock, the round, the two keys, the keys each copy
# names, the ledger, each model and package it signs and when each file
# expires, refusing any file that is not the text xtask writes and any round
# the ledger rules out. It then signs each copy, root with root.key and the
# rest with release.key, writes the raw Ed25519 signature in hex as
# <role>.sig, and logs the round. Passphrases are never echoed and reach
# openssl on a pipe. No network call. Bash 3.2 (macOS) and later; OpenSSL 3
# at a fixed path.
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
now=$(date -u +%s)
echo "this machine's clock: $(date -u +%Y-%m-%dT%H:%M:%SZ) UTC. If that is not now, press Ctrl-C."
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
hex() {
  local der
  der=$("$openssl" pkey -pubin -in "$1" -outform DER | od -An -v -tx1 | tr -d ' \n')
  case "$der" in
    302a300506032b6570032100*) [ ${#der} -eq 88 ] && printf '%s' "${der#302a300506032b6570032100}" && return ;;
  esac
  echo "$1 is not an Ed25519 public key: nothing signed" >&2
  return 1
}
rk=$(hex "$keys/root.pub")
rel=$(hex "$keys/release.pub")
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
# signed.log, beside the keys: one line for each round these keys signed. A
# release number is signed once, root once (or the same bytes again), and
# round 2 follows its own round 1.
log=$keys/signed.log
[ ! -L "$log" ] || { echo "$log is a link: nothing signed" >&2; exit 1; }
touch -- "$log"
sha() { "$openssl" dgst -sha256 -r "$1" | cut -c1-64; }
last=$(awk '$1 == 1 && $2 + 0 > m { m = $2 + 0 } END { print m + 0 }' "$log")
rooted=$(awk '{ for (i = 3; i <= NF; i++) if ($i ~ /^root=/) { print substr($i, 6); exit } }' "$log")
case "$last" in
  0) echo "signed.log: these keys have signed no release yet" ;;
  *) echo "signed.log: the last release these keys signed is $last" ;;
esac
if [ "$round" = 1 ]; then
  [ "$seq" -gt "$last" ] || { echo "release $seq is not above $last, the last these keys signed: nothing signed" >&2; exit 1; }
  [ ! -e "$tmp/root" ] || [ -z "$rooted" ] || [ "$(sha "$tmp/root")" = "$rooted" ] ||
    { echo "these keys signed another root before: nothing signed" >&2; exit 1; }
else
  case "$(tail -n 1 "$log")" in
    "1 $seq "*" snapshot=$(sha "$tmp/.snapshot") "*) ;;
    *) echo "round 1 of release $seq, with this snapshot, is not the last round these keys signed: nothing signed" >&2; exit 1 ;;
  esac
fi
# root (version 1, root.key for root and release.key for the rest), targets
# (no target of its own; *.kpkg to packages, then *.gguf and *.chunks to
# models, both release.key's), snapshot and timestamp are fixed texts at TUF
# spec 1.0.37, their dates aside: no other role can speak for a package or a
# model, and each _type is the file's own.
spec=1.0.37
stamp() { sed -e 's/"expires":"[0-9:TZ-]*"/"expires":"E"/' "$1"; }
norm() { stamp "$1" | sed "s/\"version\":$seq\([,}]\)/\"version\":N\1/g"; }
ends='"terminating":true,"threshold":1}' v='{"version":N}' sv='"spec_version":"'$spec'"'
rw='{"_type":"root","consistent_snapshot":true,"expires":"E","keys":{'$ks'},"roles":{'$(of root "$rid")','
rw=$rw$(of snapshot "$lid")','$(of targets "$lid")','$(of timestamp "$lid")'},'$sv',"version":1}'
tw='{"_type":"targets","delegations":{"keys":{'$(key "$lid" "$rel")'},"roles":[{"keyids":["'$lid'"],"name":"packages",'
tw=$tw'"paths":["*.kpkg"],'$ends',{"keyids":["'$lid'"],"name":"models","paths":["*.gguf","*.chunks"],'$ends']},'
tw=$tw'"expires":"E",'$sv',"targets":{},"version":N}'
sw='{"_type":"snapshot","expires":"E","meta":{"models.json":'$v',"packages.json":'$v',"targets.json":'$v'},'
sw=$sw$sv',"version":N}'
mw='{"_type":"timestamp","expires":"E","meta":{"snapshot.json":{"hashes":{"sha256":"'$snap'"},"length":'$size','
mw=$mw'"version":N}},'$sv',"version":N}'
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
      sv = spec; gsub(/[.]/, "[.]", sv)
      top = "^[{]\"_type\":\"targets\",\"expires\":\"[0-9:TZ-]+\",\"spec_version\":\"" sv "\",\"targets\":[{]"
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
  listed=$(awk -v role="$role" -v seq="$seq" -v spec="$spec" "$walk" "$tmp/$role") ||
    { echo "$role.canon is not what xtask writes: nothing signed" >&2; exit 1; }
  n=$(($(count "$tmp/$role" '\.kpkg":{') + $(count "$tmp/$role" '\.gguf":{') + $(count "$tmp/$role" '\.chunks":{')))
  [ "$(printf '%s' "$listed" | grep -c "^$role" || true)" = "$n" ] ||
    { echo "$role.canon lists $n targets, not each once: nothing signed" >&2; exit 1; }
  [ -z "$listed" ] || printf '%s\n' "$listed"
done
# Each role expires its full period from now, less up to the week signing may
# take: root, targets, packages and models 365 days, snapshot 30 and timestamp
# 7. This machine's clock must be right. Every file is held to its shape
# above, so its first expiry is its own, the top-level one, and it must be a
# real date.
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
      y = substr(t, 1, 4) + 0; mo = substr(t, 6, 2) + 0; d = substr(t, 9, 2) + 0
      hh = substr(t, 12, 2) + 0; mi = substr(t, 15, 2) + 0; ss = substr(t, 18, 2) + 0
      last = mo == 2 ? 28 + (y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)) : 30 + (mo + (mo > 7)) % 2
      if (mo < 1 || mo > 12 || d < 1 || d > last || hh > 23 || mi > 59 || ss > 59) {
        print name ".canon expires " t ", not a real date: nothing signed" > "/dev/stderr"
        exit 1
      }
      at = day(y, mo, d) * 86400 + hh * 3600 + mi * 60 + ss
      line = name ".canon expires " t ", in " int((at - now) / 86400 + 0.5) " days"
      if (at >= now + (days - 7) * 86400 && at <= now + (days + 1) * 86400) { print line; exit }
      print line ", not within a week of " days ": nothing signed" > "/dev/stderr"
      exit 1
    }' "$copy" || exit 1
done

# sign KEY ROLE...: asks for KEY's passphrase, checks it and that KEY.pub is
# its public key, then signs each ROLE.
sign() {
  local key="$keys/$1.key" pass sig name want=$rel
  case "$1" in root) want=$rk ;; esac
  shift
  [ $# -gt 0 ] || return 0
  IFS= read -rsp "Passphrase for $(basename "$key"): " pass
  echo >&2
  printf '%s\n' "$pass" | "$openssl" pkey -in "$key" -passin stdin -pubout > "$tmp/.$(basename "$key").pub" ||
    { echo "not the passphrase of $key: nothing signed with it" >&2; exit 1; }
  [ "$(hex "$tmp/.$(basename "$key").pub")" = "$want" ] ||
    { echo "$key is not the key its .pub names: nothing signed with it" >&2; exit 1; }
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
# The ledger's line for this round, once every file is signed.
line="$round $seq"
for copy in "$@"; do line="$line $(basename "$copy")=$(sha "$copy")"; done
printf '%s\n' "$line" >> "$log"
echo "signed.log: round $round of release $seq"
