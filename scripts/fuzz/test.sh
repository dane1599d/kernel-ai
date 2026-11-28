#!/usr/bin/env bash
# Checks scripts/fuzz/bins.awk on hand-written tables and on fuzz/Cargo.toml:
# every file in fuzz/fuzz_targets has a [[bin]] with that file as its path.
# Usage: bash scripts/fuzz/test.sh   (needs awk and bash only)
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
fails=0

check() {  # $1 description, $2 expected output, $3 the table text
  printf '%b' "$3" > "$T/Cargo.toml"
  local got
  got=$(awk -f "$HERE/bins.awk" "$T/Cargo.toml")
  if [ "$got" = "$2" ]; then echo "ok   $1"; else
    echo "FAIL $1: got '$got', wanted '$2'"; fails=$((fails+1)); fi
}

check "spaces around =" "url fuzz_targets/url.rs" \
  '[[bin]]\nname = "url"\npath = "fuzz_targets/url.rs"\n'
check "no spaces around =" "url fuzz_targets/url.rs" \
  '[[bin]]\nname="url"\npath="fuzz_targets/url.rs"\n'
check "single quotes, wide spaces and a comment" "tls fuzz_targets/tls.rs" \
  "[[bin]]\nname   =  'tls'  # the record layer\npath = 'fuzz_targets/tls.rs'\n"
check "path before name" "b fuzz_targets/b.rs" \
  '[[bin]]\npath = "fuzz_targets/b.rs"\nname = "b"\n'
check "other tables' keys never count" "a fuzz_targets/a.rs" \
  '[package]\nname = "kainet-fuzz"\n\n[[bin]]\nname = "a"\npath = "fuzz_targets/a.rs"\ntest = false\n\n[dependencies]\npath = "x"\n'
check "a bin with no path is absent" "" \
  '[[bin]]\nname = "c"\n\n[dependencies]\npath = "x"\n'
check "indented keys" "e fuzz_targets/e.rs" \
  '[[bin]]\n  name = "e"\n\tpath = "fuzz_targets/e.rs"\n'
check "a dash in the name" "dns-name fuzz_targets/dns-name.rs" \
  '[[bin]]\nname = "dns-name"\npath = "fuzz_targets/dns-name.rs"\n'
check "one [[bin]] right after another" "f fuzz_targets/f.rs
g fuzz_targets/g.rs" \
  '[[bin]]\nname = "f"\npath = "fuzz_targets/f.rs"\n[[bin]]\nname = "g"\npath = "fuzz_targets/g.rs"\n'
check "a # inside quotes is kept" "h fuzz_targets/h#1.rs" \
  '[[bin]]\nname = "h"  # a comment\npath = "fuzz_targets/h#1.rs" # another\n'
check "a commented-out [[bin]] is absent" "" \
  '# [[bin]]\n# name = "d"\n# path = "fuzz_targets/d.rs"\n'
check "an escaped quote and backslash in a \"...\" value" 'q"1 fuzz_targets/q\1.rs' \
  '[[bin]]\nname = "q\\"1"\npath = "fuzz_targets/q\\\\1.rs" # "x"\n'
check "a '...' value has no escapes" 'r\\ fuzz_targets/r.rs' \
  "[[bin]]\nname = 'r\\\\\\\\'\npath = 'fuzz_targets/r.rs'\n"

fails_on() {  # $1 description, $2 the table text: bins.awk must exit 1 and print nothing
  printf '%b' "$2" > "$T/Cargo.toml"
  local got status
  got=$(awk -f "$HERE/bins.awk" "$T/Cargo.toml" 2>/dev/null); status=$?
  if [ "$status" = 1 ] && [ -z "$got" ]; then echo "ok   $1"; else
    echo "FAIL $1: exit $status, printed '$got'"; fails=$((fails+1)); fi
}
fails_on "a \" value with no closing quote fails" \
  '[[bin]]\nname = "a"\npath = "fuzz_targets/a.rs\n[[bin]]\nname = "b"\npath = "fuzz_targets/b.rs"\n'
fails_on "a ' value with no closing quote fails" \
  "[[bin]]\nname = 'a\npath = 'fuzz_targets/a.rs'\n"
fails_on "an escaped closing quote does not close" \
  '[[bin]]\nname = "a\\"\npath = "fuzz_targets/a.rs"\n'

bins=$(awk -f "$HERE/bins.awk" "$ROOT/fuzz/Cargo.toml")
for f in "$ROOT"/fuzz/fuzz_targets/*.rs; do
  t=$(basename "$f" .rs)
  if echo "$bins" | grep -qxF "$t fuzz_targets/$t.rs"; then :; else
    echo "FAIL fuzz/Cargo.toml has no [[bin]] for $t"; fails=$((fails+1)); fi
done
echo "ok   fuzz/Cargo.toml: $(echo "$bins" | wc -l | tr -d ' ') bins read"

[ "$fails" = 0 ] || { echo "$fails failed"; exit 1; }
