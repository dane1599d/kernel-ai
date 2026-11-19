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

# Whether code that is not itself a test or a tool reads `$1` at build time:
# a .rs file outside the tests/ directories, testkit/ and the xtask tools
# below that names the file and uses include_str!, include_bytes! or #[path]
# anywhere (so a concat!(env!("CARGO_MANIFEST_DIR"), "...") over several lines
# counts). Matched by file name, as fixed text. Any error answers yes: the
# rule fails closed.
referenced() {  # $1 path
  local name files rc
  name=$(basename "$1")
  files=$(git grep -l -F -e "$name" "$head" -- '*.rs' 2>/dev/null); rc=$?
  [ $rc = 1 ] && return 1
  [ $rc = 0 ] || return 0
  files=$(printf '%s\n' "$files" | cut -d: -f2- | awk '
    /^testkit\// { next }
    /^xtask\/src\/(decisions|filesize|actbench|tokens|doctor)/ { next }
    /(^|\/)tests\// && !/\/src\// { next }
    { print }')
  [ -n "$files" ] || return 1
  # Exit 0 (read) at the first file that includes, or on any error.
  if printf '%s\n' "$files" | {
    while IFS= read -r g; do
      git grep -q -E 'include_(str|bytes)!|#\[path' "$head" -- "$g" 2>/dev/null
      [ $? = 1 ] || exit 0
    done
    exit 1
  }; then
    return 0
  fi
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
