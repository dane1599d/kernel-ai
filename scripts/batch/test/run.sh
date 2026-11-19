#!/usr/bin/env bash
# Runs batch-ff.sh and batch-auto.sh against a local bare repository and a stub
# gh (test/gh), with sleep stubbed out: a normal batch, the one-model-PR rule,
# a stop before and after the fast-forward with a resume, a resume refused
# because main or a PR head moved, and the head-branch deletion guards.
# Usage: bash scripts/batch/test/run.sh   (needs git and bash only; ~1 minute)
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
B=$(dirname "$HERE")
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/bin"
cp "$HERE/gh" "$HERE/sleep" "$T/bin/" && chmod +x "$T/bin/"*
export STUB_T="$T" PATH="$T/bin:$PATH" KAI_GIT_DIR="$T/work" KAI_BATCH_STATE="$T/state"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
fails=0

check() {  # $1 description, $2 command
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fails=$((fails+1)); fi
}
has() { grep -qF -- "$1" "$T/out"; }
branches() { git -C "$T/origin.git" for-each-ref --format='%(refname:short)' refs/heads | tr '\n' ' '; }
gone() { ! git -C "$T/origin.git" rev-parse -q --verify "refs/heads/$1" >/dev/null; }

# A batch-ff run the stub can kill (kill-on-ready: just before the fast-forward;
# kill-on-merged: just after it), as a PC stop would.
ff() { bash "$B/batch-ff.sh" "$@" > "$T/out" 2>&1 & echo $! > "$T/ffpid"; wait 2>/dev/null; }

mkpr() {  # n branch base files...
  local n=$1 br=$2 b=$3 f; shift 3
  git -C "$T/work" checkout -q -B "$br" "origin/$b"
  for f in "$@"; do mkdir -p "$T/work/$(dirname "$f")"; echo "$br" > "$T/work/$f"; done
  git -C "$T/work" add -A && git -C "$T/work" commit -qm "$br" && git -C "$T/work" push -q origin "$br"
  git -C "$T/work" fetch -q origin
  mkdir -p "$T/prs/$n"
  echo OPEN > "$T/prs/$n/state"; echo "$br" > "$T/prs/$n/branch"; echo "$b" > "$T/prs/$n/base"
  printf '%s\n' "$@" > "$T/prs/$n/files"; echo batch-ready > "$T/prs/$n/labels"
  git -C "$T/work" rev-parse HEAD > "$T/prs/$n/sha"
}

setup() {
  rm -rf "$T/origin.git" "$T/work" "$T/prs" "$T/state" "$T"/kill-* "$T/fluency-dispatches" "$T/list-fails"
  git init -q --bare "$T/origin.git"
  git -C "$T/origin.git" symbolic-ref HEAD refs/heads/main
  git clone -q "$T/origin.git" "$T/work" 2>/dev/null
  echo base > "$T/work/base.txt"
  git -C "$T/work" add -A && git -C "$T/work" commit -qm base && git -C "$T/work" push -q origin HEAD:main
  git -C "$T/work" fetch -q origin
  mkpr 1 feat/a main a.txt
  mkpr 2 feat/m1 main infer/src/x.rs
  mkpr 3 feat/m2 main quant/y.rs
  mkpr 4 feat/c feat/m2 c.txt
  mkpr 5 feat/d main d.txt
  mkpr 6 feat/e feat/d e.txt; : > "$T/prs/6/labels"
  mkpr 7 release/v0.1 main r.txt
  git -C "$T/work" checkout -q --detach origin/main
  printf 'fluency: 48/50 first try (96%%), 49/50 after one repair (98%%)\nthe fluency gate passes\n' > "$T/fluency.log"
}

echo "== a fresh batch"
setup
ff 1 2 3 4 5 7
check "the first model PR goes in"            'has "  + #2 (feat/m1)"'
check "a second model PR waits"               'has "  - #3 changes what the model sees, and so does #2"'
check "its stacked child waits with it"       'has "  - #4 is stacked on a left-out PR"'
check "the fluency gate ran once"             '[ "$(wc -l < "$T/fluency-dispatches")" = 1 ]'
check "main holds the tested batch"           'has "main fast-forwarded to the tested batch"'
check "landed heads are deleted"              'gone feat/a && gone feat/m1'
check "the base of an open PR is kept"        'has "kept feat/d (an open PR" && ! gone feat/d'
check "release/* is kept"                     'has "kept release/v0.1 (protected)" && ! gone release/v0.1'
check "no resume state is left"               '[ ! -f "$T/state/current-batch" ]'

echo "== a stop just before the fast-forward, then --resume"
touch "$T/kill-on-ready"; ff 3 4
check "the stop leaves a green state"         'grep -q "^phase=green" "$T/state/current-batch"'
ff --resume
check "the resume lands both"                 'has "  #3: MERGED" && has "  #4: MERGED"'
check "the resume did not re-run fluency"     '[ "$(wc -l < "$T/fluency-dispatches")" = 2 ]'
check "the resume deletes the heads"          'gone feat/m2 && gone feat/c'
check "the state is cleared"                  '[ ! -f "$T/state/current-batch" ]'

echo "== a stop just after the fast-forward, then --resume"
setup
touch "$T/kill-on-merged"; ff 1 5
ff --resume
check "the resume sees main already holds it" 'has "main already fast-forwarded to the tested batch"'
check "and finishes the tidying"              'has "  #1: MERGED" && gone feat/a && [ ! -f "$T/state/current-batch" ]'

echo "== a resume refuses when main moved"
touch "$T/kill-on-ready"; ff 2
sed -i 's/^phase=.*/phase=pushed/' "$T/state/current-batch"
git -C "$T/work" fetch -q origin && git -C "$T/work" checkout -q --detach origin/main
echo x > "$T/work/x.txt"; git -C "$T/work" add -A; git -C "$T/work" commit -qm moved; git -C "$T/work" push -q origin HEAD:main
ff --resume
check "main moved: nothing lands"             'has "main moved during the batch; STOPPING" && [ ! -f "$T/state/current-batch" ]'

echo "== a resume refuses when a PR head moved"
git -C "$T/work" fetch -q origin
touch "$T/kill-on-ready"; ff 2
git -C "$T/work" checkout -q feat/m1 && git -C "$T/work" reset -q --hard origin/feat/m1
echo y >> "$T/work/infer/src/x.rs"; git -C "$T/work" commit -qam more; git -C "$T/work" push -q origin feat/m1
ff --resume
check "head moved: nothing lands"             'has "#2 was pushed to during the batch; STOPPING" && [ ! -f "$T/state/current-batch" ]'

echo "== no branch is deleted when the open PRs cannot be listed"
git -C "$T/work" fetch -q origin; git -C "$T/origin.git" rev-parse feat/m1 > "$T/prs/2/sha"
touch "$T/list-fails"; ff 2; rm -f "$T/list-fails"
check "landed, branch kept"                   'has "  #2: MERGED" && has "could not list open PRs" && ! gone feat/m1'

echo "== a fork's branch is kept"
setup
echo true > "$T/prs/1/cross"; ff 1
check "landed, branch kept"                   'has "kept feat/a (#1: MERGED true)" && ! gone feat/a'

echo "== a resume refuses a PR unlabelled since the stop"
setup
touch "$T/kill-on-ready"; ff 1
: > "$T/prs/1/labels"
ff --resume
check "unlabelled: nothing lands"             'has "#1 is no longer batch-ready; STOPPING" && ! gone feat/a && [ ! -f "$T/state/current-batch" ]'

echo "== a damaged resume state cancels and deletes nothing"
printf 'phase=pushed\nname=\nbase=x\nc=y\nbpr=\nprs=1\nheads_s=x\nbrs_s=feat/a\n' > "$T/state/current-batch"
: > "$T/gh.log"; ff --resume
check "no batch branch: stopped"              'has "names no batch branch; STOPPING" && [ ! -f "$T/state/current-batch" ]'
check "and no run was listed or cancelled"    '! grep -q "run " "$T/gh.log"'

echo "== batch-auto.sh resumes an unfinished batch first"
setup
touch "$T/kill-on-ready"; ff 1 2
bash "$B/batch-auto.sh" & A=$!
for i in $(seq 1 120); do grep -q "#2 merged" "$T/state/batch-auto.log" 2>/dev/null && break; /bin/sleep 1 2>/dev/null || /usr/bin/sleep 1; done
touch "$T/state/batch-auto.stop"; wait $A
check "the log shows the resume and merges"   'grep -q "resuming the unfinished batch: 1 2" "$T/state/batch-auto.log" && grep -q "#1 merged" "$T/state/batch-auto.log"'

echo "== batch-auto.sh: a lone model PR that fails fluency leaves; the rest land"
setup
printf 'fluency: 40/50 first try (80%%), 42/50 after one repair (84%%)\nthe fluency gate fails\n' > "$T/fluency.log"
bash "$B/batch-auto.sh" > /dev/null & A=$!
for i in $(seq 1 120); do grep -q "#7 merged" "$T/state/batch-auto.log" 2>/dev/null && break; /bin/sleep 1 2>/dev/null || /usr/bin/sleep 1; done
touch "$T/state/batch-auto.stop"; wait $A
L="$T/state/batch-auto.log"
check "the model PR loses its label"          'grep -q "#2 failed the fluency gate" "$L" && [ ! -s "$T/prs/2/labels" ]'
check "the others land without a split"       'grep -q "#1 merged" "$L" && grep -q "#5 merged" "$L" && grep -q "#7 merged" "$L" && ! grep -q "splitting" "$L"'
check "the model PR is not merged"            '! grep -q "#2 merged" "$L" && ! gone feat/m1'

echo "branches left: $(branches)"
[ $fails = 0 ] && echo "all batcher tests pass" || { echo "$fails batcher test(s) failed"; exit 1; }
