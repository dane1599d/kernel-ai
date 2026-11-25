#!/usr/bin/env bash
# The no-boot rule: can a change reach the OS image or what the boot jobs run?
# Usage: noboot.sh <base> <head>   (revisions in the current checkout)
# Looks at every file changed between <head> and its merge base with <base>
# (so a PR stacked on another carries its parent's files too), old and new
# names of a rename both. Exit 0 when no file needs the boot jobs; exit 1 and
# print those that do; exit 2 when the revisions cannot be read.
#
# Conservative by path: a file needs the boot jobs if code that is not a test
# reads it at build time (named in a .rs file that uses include_str!,
# include_bytes! or #[path]; see `referenced`), and otherwise unless it is one of
# - documentation: under docs/ or ending in .md, outside every src/ (xtask
#   packs a component's whole src/ into the image);
# - a crate's integration tests: <crate>/tests/... beside the crate's
#   Cargo.toml, which no binary of the image links;
# - testkit/, the shared hostile-input helpers only tests reach by #[path];
# - scripts/ (release signing and other host tools), except scripts/batch/:
#   the batcher and this rule never land through the lane they decide;
# - .github/, except ci.yml and .github/actions/, which define the boot jobs;
# - an xtask tool no boot run calls: decisions, filesize, actbench, tokens,
#   doctor.
# Everything else (the kernel, user programs, components, the crates they
# link, wit/, xtask's runner, cases and image code, Cargo.toml and
# Cargo.lock, toolchain files) needs them.
set -u
base=$1 head=$2
mb=$(git merge-base "$base" "$head" 2>/dev/null) || { echo "noboot: cannot read $base and $head"; exit 2; }
files=$(git diff --name-only --no-renames "$mb" "$head") || exit 2

# The git that `referenced` greps with; a test points it at one that fails.
GIT=${NOBOOT_GIT:-git}
# How many hops through test-only files (tests/, testkit/, the tools) a read
# is followed back towards image code.
HOPS=4

# Whether a file is test-only or a tool's: what it reads counts only when
# image code reads it in turn.
test_only() {  # $1 path
  case "$1" in
    testkit/*) return 0;;
    xtask/src/decisions.rs|xtask/src/decisions/*|xtask/src/filesize.rs) return 0;;
    xtask/src/actbench/*|xtask/src/tokens.rs|xtask/src/doctor/*) return 0;;
    */src/*) return 1;;
    tests/*|*/tests/*) return 0;;
  esac
  return 1
}

# The .rs files at $head that may read `$1` at build time: those that name
# it (fixed text) and use include!, include_str!, include_bytes! or #[path]
# anywhere in them (so a concat!(env!("CARGO_MANIFEST_DIR"), "...") over
# several lines counts), and those that #[path] the module whose tree holds
# it (`<dir>/mod.rs` or `<dir>.rs` for any parent <dir> of it). Prints them;
# exits 3 on any error.
readers() {  # $1 path
  local name dir files rc g
  name=$(basename "$1")
  files=$($GIT grep -l -F -e "$name" "$head" -- '*.rs' 2>/dev/null); rc=$?
  [ $rc -gt 1 ] && return 3
  printf '%s\n' "$files" | cut -d: -f2- | while IFS= read -r g; do
    [ -n "$g" ] || continue
    $GIT grep -q -E 'include(_str|_bytes)?!|#\[path' "$head" -- ":(literal)$g" 2>/dev/null; rc=$?
    [ $rc = 0 ] && echo "$g"
    [ $rc -gt 1 ] && echo "?error"
  done
  # A #[path]ed `<dir>/mod.rs` or `<dir>.rs` brings the module tree in <dir>/.
  dir=$(dirname "$1")
  while [ "$dir" != . ] && [ "$dir" != / ]; do
    name=$(basename "$dir" | sed 's/[.[\*^$+?(){}|]/\\&/g')
    files=$($GIT grep -l -E "#\[path *= *\"([^\"]*/)?$name(/mod\.rs|\.rs)\"" "$head" -- '*.rs' 2>/dev/null); rc=$?
    [ $rc -gt 1 ] && { echo "?error"; break; }
    printf '%s\n' "$files" | cut -d: -f2- | grep -v '^$'
    dir=$(dirname "$dir")
  done
}

# Whether image code (anything not test-only) reads `$1` at build time,
# directly or through up to $HOPS test-only files that it reads in turn.
# Any error answers yes: the rule fails closed.
referenced() {  # $1 path, $2 hops left (default $HOPS)
  local left=${2:-$HOPS} out r
  out=$(readers "$1"); [ $? = 0 ] || return 0
  printf '%s\n' "$out" | grep -qx '?error' && return 0
  while IFS= read -r r; do
    [ -n "$r" ] && [ "$r" != "$1" ] || continue
    test_only "$r" || return 0
    [ "$left" -gt 0 ] && referenced "$r" $((left - 1)) && return 0
  done <<< "$out"
  return 1
}

needs_boot() {  # $1 path
  local f=$1 crate
  case "$f" in
    # The batcher and this rule never land through the lane they decide.
    scripts/batch/*) return 0;;
  esac
  # A file image code reads at build time is part of the image, whatever
  # its path (a doc, a test fixture, a testkit helper).
  referenced "$f" && return 0
  case "$f" in
    xtask/src/decisions.rs|xtask/src/decisions/*|xtask/src/filesize.rs) return 1;;
    xtask/src/actbench/*|xtask/src/tokens.rs|xtask/src/doctor/*) return 1;;
    # Any file under a src/ is a crate's: xtask packs everything under a
    # component's src/, whatever its kind, into the image as its kai.source
    # (components.rs, collect), so not even a .md there is documentation.
    src/*|*/src/*) return 0;;
    docs/*|*.md) return 1;;
    testkit/*|scripts/*) return 1;;
    .github/workflows/ci.yml|.github/actions/*) return 0;;
    .github/*) return 1;;
    */tests/*)
      crate=${f%%/tests/*}
      git cat-file -e "$head:$crate/Cargo.toml" 2>/dev/null && return 1
      return 0;;
  esac
  return 0
}

bad=0
while IFS= read -r f; do
  [ -n "$f" ] || continue
  if needs_boot "$f"; then
    [ $bad -lt 10 ] && echo "needs the boot jobs: $f"
    bad=$((bad + 1))
  fi
done <<< "$files"
[ $bad -eq 0 ] || { [ $bad -gt 10 ] && echo "... and $((bad - 10)) more"; exit 1; }
exit 0
