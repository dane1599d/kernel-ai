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
  rm -rf "$T/origin.git" "$T/work" "$T/prs" "$T/state" "$T"/kill-* "$T/fluency-dispatches" "$T/list-fails" "$T/others" "$T/others-fail" "$T/fail-with" "$T/compile.log" "$T"/on-ci-wait-* "$T/ci-waits" "$T/on-fluency-wait" "$T/ready-hook" "$T/fluency-hook" "$T/on-ready" "$T/moved-head" "$T/edit-fails" "$T/ffpid"
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
check "dispatched with CI, before it is read" 'awk "/workflow run fluency-model/ && !w {w=NR} /run list .*--workflow ci.yml/ && !c {c=NR} END {exit !(w && c && w < c)}" "$T/gh.log"'
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

echo "== a model batch whose CI fails: its fluency run is cancelled with it"
setup
: > "$T/gh.log"
CI_RESULT=failure ff 2
check "dispatched, then the batch's runs cancelled" 'grep -q "workflow run fluency-model.yml --repo .* --ref batch/" "$T/gh.log" && has "batch NOT green" && grep -q "run cancel 789" "$T/gh.log" && ! has "#2: MERGED"'

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

echo "== ci.yml: every job after another guards it explicitly"
# A job's implicit success() covers its needs' needs too (actions/runner#491):
# with `noboot` skipped on every run but a no-boot batch, a boot job without an
# explicit guard would be skipped, and the batcher would read every batch as
# red. So every job with `needs:` says `!cancelled()`, and checks each need it
# relies on by `needs.<job>.result == 'success'` (all but `noboot`, which
# `qemu` reads by its output and runs past when it is skipped, and `prebuild`,
# whose output the boot shards read and which they run past when it fails).
CI="$B/../../.github/workflows/ci.yml"
guards=$(awk '
  /^  [a-z][a-z0-9_-]*:$/ { if (job != "") print job "|" needs "|" cond; job = substr($1, 1, length($1) - 1); needs = ""; cond = ""; inif = 0; next }
  /^[a-z]/ { if (job != "") print job "|" needs "|" cond; job = ""; next }
  job == "" { next }
  /^    needs: / { needs = $0; sub(/^    needs: */, "", needs); gsub(/[][ ]/, "", needs); inif = 0; next }
  /^    if: / { cond = $0; inif = ($0 ~ /: >-$/); next }
  /^    [a-z]/ { inif = 0; next }
  inif && /^      / { cond = cond " " $0; next }
  END { if (job != "") print job "|" needs "|" cond }' "$CI")
bad_guard=""
while IFS='|' read -r job needs cond; do
  [ -n "$needs" ] || continue
  case "$cond" in *'!cancelled()'*) ;; *) bad_guard="$bad_guard $job(!cancelled)";; esac
  for n in $(echo "$needs" | tr ',' ' '); do
    case "$n" in noboot|prebuild) continue;; esac
    case "$cond" in *"needs.$n.result == 'success'"*) ;; *) bad_guard="$bad_guard $job(needs.$n)";; esac
  done
done <<< "$guards"
check "jobs with needs are listed"            'echo "$guards" | grep -q "^boot|qemu,prebuild|" && echo "$guards" | grep -q "^observe-compare|observe|"'
check "each guards its needs explicitly"      '[ -z "$bad_guard" ] || { echo "  missing:$bad_guard"; false; }'

echo "== noboot.sh: every arm of the rule"
# A repo of its own: each case is one commit on a branch from `base`, judged
# by noboot.sh as CI judges a batch (exit 0 no-boot, 1 needs boot, 2 unreadable).
RT="$T/rule"
rm -rf "$RT"; git init -q "$RT"
mkdir -p "$RT/crate/src" "$RT/apps/x/src" "$RT/kernel/src" "$RT/docs"
for f in crate/Cargo.toml apps/x/Cargo.toml kernel/Cargo.toml Cargo.lock kernel/src/a.rs docs/inc.md; do echo x > "$RT/$f"; done
printf 'const D: &str = include_str!("../../docs/inc.md");\n' > "$RT/crate/src/lib.rs"
# Image code reading files the path arms would allow: a doc through
# concat!(env!(...)) over several lines, a test fixture, a testkit helper.
printf 'const C: &[u8] = include_bytes!(concat!(\n    env!("CARGO_MANIFEST_DIR"),\n    "/../docs/cc.md"\n));\n' > "$RT/kernel/src/c.rs"
printf 'const F: &[u8] = include_bytes!("../../crate/tests/fixture.bin");\n' > "$RT/kernel/src/f.rs"
printf '#[path = "../../testkit/used.rs"]\nmod used;\n' > "$RT/kernel/src/k.rs"
# A test reaching another test file: not image code.
mkdir -p "$RT/crate/tests"; printf '#[path = "b.rs"]\nmod b;\n' > "$RT/crate/tests/a.rs"
# include! itself; a #[path]ed mod.rs, whose module tree comes with it; a
# testkit file image code reaches that reads a test fixture in turn.
printf 'include!("../../docs/gen.md");\n' > "$RT/kernel/src/i.rs"
printf '#[path = "../../testkit/tree/mod.rs"]\nmod tree;\n' > "$RT/kernel/src/m.rs"
mkdir -p "$RT/testkit/tree"; printf 'mod leaf;\n' > "$RT/testkit/tree/mod.rs"; echo x > "$RT/testkit/tree/leaf.rs"
printf '#[path = "../../testkit/hop.rs"]\nmod hop;\n' > "$RT/kernel/src/h.rs"
printf 'const X: &str = include_str!("../crate/tests/data.txt");\n' > "$RT/testkit/hop.rs"
git -C "$RT" add -A && git -C "$RT" commit -qm base && git -C "$RT" branch -q base
rule() {  # $1 expected exit, $2 description, then paths to write
  local want=$1 what=$2 got; shift 2
  git -C "$RT" checkout -q -B case base
  for f in "$@"; do mkdir -p "$RT/$(dirname "$f")"; echo y >> "$RT/$f"; done
  git -C "$RT" add -A && git -C "$RT" commit -qm case
  (cd "$RT" && bash "$B/noboot.sh" base case > /dev/null); got=$?
  check "$what (exit $want)" '[ "$got" = "$want" ]'
}
rule 0 "a doc under docs/"                    docs/plan.md
rule 0 "a .md at the root"                    README.md
rule 1 "a doc a .rs include_str!s"            docs/inc.md
rule 1 "a doc read through concat!(env!(..))" docs/cc.md
rule 1 "a tests/ fixture image code includes" crate/tests/fixture.bin
rule 1 "a testkit/ file image code #[path]s"  testkit/used.rs
rule 0 "a tests/ file only a test reaches"    crate/tests/b.rs
rule 1 "a doc image code include!s"           docs/gen.md
rule 1 "a file in a #[path]ed mod.rs's tree"  testkit/tree/leaf.rs
rule 1 "a fixture a testkit helper reads"     crate/tests/data.txt
rule 1 "a .md under a component's src/"       apps/x/src/notes.md
rule 0 "a crate's tests/ beside Cargo.toml"   crate/tests/t.rs
rule 1 "a tests/ with no Cargo.toml beside"   loose/tests/t.rs
rule 1 "a src/tests/ module"                  crate/src/tests/t.rs
rule 0 "testkit/"                             testkit/h.rs
rule 0 "a script outside scripts/batch"       scripts/tool.sh
rule 1 "the batcher or its rule"              scripts/batch/noboot.sh
rule 0 "another workflow"                     .github/workflows/other.yml
rule 1 "ci.yml"                               .github/workflows/ci.yml
rule 1 "a CI action"                          .github/actions/qemu/action.yml
rule 0 "an xtask tool no boot run calls"      xtask/src/tokens.rs
rule 1 "xtask's runner"                       xtask/src/main.rs
rule 1 "Cargo.lock"                           Cargo.lock
rule 1 "kernel code"                          kernel/src/a.rs
rule 1 "a doc and kernel code together"       docs/plan.md kernel/src/b.rs
git -C "$RT" checkout -q -B case base && git -C "$RT" mv kernel/src/a.rs docs/a.md && git -C "$RT" commit -qm rename
(cd "$RT" && bash "$B/noboot.sh" base case > /dev/null); got=$?
check "a rename counts its old name (exit 1)"  '[ "$got" = 1 ]'
git -C "$RT" checkout -q -B parent base && echo z >> "$RT/kernel/src/a.rs" && git -C "$RT" commit -qam parent
git -C "$RT" checkout -q -b child && echo z > "$RT/docs/child.md" && git -C "$RT" add -A && git -C "$RT" commit -qm child
(cd "$RT" && bash "$B/noboot.sh" base child > /dev/null); got=$?
check "a docs PR stacked on a kernel PR (exit 1)" '[ "$got" = 1 ]'
(cd "$RT" && bash "$B/noboot.sh" base no-such-rev > /dev/null 2>&1); got=$?
check "unreadable revisions (exit 2)"          '[ "$got" = 2 ]'
# A git whose grep fails: the reference check must answer "read" (fail closed).
printf '#!/bin/sh\n[ "$1" = grep ] && exit 2\nexec git "$@"\n' > "$T/failgit"; chmod +x "$T/failgit"
git -C "$RT" checkout -q -B case base && echo y >> "$RT/docs/plan.md" && git -C "$RT" add -A && git -C "$RT" commit -qm case
(cd "$RT" && NOBOOT_GIT="$T/failgit" bash "$B/noboot.sh" base case > /dev/null); got=$?
check "a grep error fails closed (exit 1)"     '[ "$got" = 1 ]'

echo "== another workflow still running, or unlisted, is not green"
setup
echo "kaifs=in_progress" > "$T/others"
ff 1
check "unfinished: CI did not finish"         'has "a workflow did not finish" && has "CI did not finish (another workflow); STOPPING" && ! has "#1: MERGED" && [ -s "$T/prs/1/labels" ]'
echo "kaifs=queued" > "$T/others"
ff 1
check "queued: CI did not finish"             'has "a workflow did not finish" && ! has "#1: MERGED" && [ -s "$T/prs/1/labels" ]'
rm -f "$T/others"; touch "$T/others-fail"
ff 1
check "unlisted: CI did not finish"           'has "the workflow listing failed" && has "CI did not finish (another workflow)" && ! has "#1: MERGED"'
rm -f "$T/others-fail"; : > "$T/others"
ff 1
check "no other workflow at all: lands"       'has "  #1: MERGED"'

echo "== all_checks (the label watches): only finished, passed checks are fine"
# gh pr checks as bucket|name rows, the form all_checks asks gh for.
ac() { ( stub_rows=$1; R=x; gh() { printf '%s' "$stub_rows"; }; . "$B/checks-lib.sh"; all_checks 1 ) }
P='pass|Format, lint, host unit tests'
check "all passed: ok"                        '[ "$(ac "$P"$'"'"'\nskipping|Boot test'"'"')" = ok ]'
check "nothing listed yet: pending"           '[ "$(ac "")" = pending ]'
check "a workflow running: pending"           '[ "$(ac "$P"$'"'"'\npending|oracle'"'"')" = pending ]'
check "an unknown state: pending"             '[ "$(ac "$P"$'"'"'\nqueued|oracle'"'"')" = pending ]'
check "a failure: fail"                       'case "$(ac "$P"$'"'"'\nfail|oracle'"'"')" in "fail: oracle; ") true;; *) false;; esac'
check "cancelled, never passed: fail"         'case "$(ac "$P"$'"'"'\ncancel|oracle'"'"')" in *"never passed: oracle; ") true;; *) false;; esac'
check "cancelled twin of a passed run: ok"    '[ "$(ac "$P"$'"'"'\ncancel|Format, lint, host unit tests'"'"')" = ok ]'

echo "== a no-boot batch takes only PRs the boot jobs cannot see"
setup
mkpr 8 feat/doc main docs/x.md
mkpr 9 feat/tool main scripts/y.sh
git -C "$T/work" checkout -q --detach origin/main
: > "$T/gh.log"
bash "$B/batch-ff.sh" --noboot 1 8 9 > "$T/out" 2>&1
check "a boot PR is left out, label kept"     'has "#1 changes what the boot jobs check; left out of a no-boot batch" && [ -s "$T/prs/1/labels" ]'
check "the others land on a noboot branch"    'has "  #8: MERGED" && has "  #9: MERGED" && grep -q "pr create .*--head batch/noboot-" "$T/gh.log"'

# batch-auto.sh until $1 shows in its log, then stopped; extra env before it.
auto_until() {  # $1 text to wait for
  bash "$B/batch-auto.sh" > /dev/null & A=$!
  for i in $(seq 1 120); do grep -q "$1" "$T/state/batch-auto.log" 2>/dev/null && break; /bin/sleep 1 2>/dev/null || /usr/bin/sleep 1; done
  touch "$T/state/batch-auto.stop"; wait $A
  L="$T/state/batch-auto.log"
}
nb_setup() {  # PRs 8 (a doc) and 9 (a script) beside the usual ones; labels on $@ only
  setup
  mkpr 8 feat/doc main docs/x.md
  mkpr 9 feat/tool main scripts/y.sh
  git -C "$T/work" checkout -q --detach origin/main
  for n in 1 2 3 4 5 7 8 9; do case " $* " in *" $n "*) ;; *) : > "$T/prs/$n/labels";; esac; done
}

echo "== batch-auto.sh: only no-boot PRs eligible: a no-boot batch"
nb_setup 8 9
: > "$T/gh.log"
auto_until "#9 merged"
check "a no-boot batch of 8 and 9"            'grep -q "no-boot batch: 8 9" "$L" && grep -q "#8 merged" "$L" && grep -q "pr create .*--head batch/noboot-" "$T/gh.log"'

echo "== batch-auto.sh: a boot PR waiting too: one full batch for all"
nb_setup 1 8 9
: > "$T/gh.log"
auto_until "#1 merged"
check "no no-boot batch while a boot PR waits" '! grep -q "no-boot batch" "$L" && grep -q "batch: 1 8 9$" "$L" && grep -q "#8 merged" "$L"'
check "and its branch is a full one"          '! grep -q -- "--head batch/noboot-" "$T/gh.log"'

echo "== batch-auto.sh: a batch-first boot PR goes in a full batch, first"
nb_setup 1 8 9
printf 'batch-ready\nbatch-first\n' > "$T/prs/1/labels"
: > "$T/gh.log"
auto_until "#1 merged"
check "batch-first boot PR: one full batch"   '! grep -q "no-boot batch" "$L" && grep -q "batch: 1 8 9$" "$L" && ! grep -q -- "--head batch/noboot-" "$T/gh.log"'

echo "== batch-auto.sh: KAI_BATCH_NOBOOT=0 turns the lane off"
nb_setup 8 9
KAI_BATCH_NOBOOT=0 auto_until "#9 merged"
check "off: a full batch even of no-boot PRs" '! grep -q "no-boot batch" "$L" && grep -q "batch: 8 9$" "$L"'

echo "== batch-auto.sh: a failed no-boot batch is tried once as a full batch"
nb_setup 8
CI_RESULT=failure auto_until "failed alone"
check "a lone PR gets its full batch first"   'grep -q "no-boot batch of 8 failed; trying it once as a full batch" "$L" && grep -q "#8 failed alone" "$L"'

echo "== batch-auto.sh: a resumed no-boot batch that fails is tried once as a full batch"
nb_setup 8
touch "$T/kill-on-ready"; ff --noboot 8
sed -i 's/^phase=.*/phase=pushed/' "$T/state/current-batch"
CI_RESULT=failure auto_until "failed alone"
check "the resume's failure gets its full batch" 'grep -q "resuming the unfinished batch: 8" "$L" && grep -q "no-boot batch of 8 failed; trying it once as a full batch" "$L" && grep -q "#8 failed alone" "$L"'

echo "== batch-ff.sh: a no-boot batch whose combined commit needs boot goes full"
setup
mkpr 8 feat/doc main docs/x.md
git -C "$T/work" checkout -q --detach origin/main
mkdir -p "$T/work/kernel/src"; printf 'const D: &str = include_str!("../../docs/x.md");\n' > "$T/work/kernel/src/inc.rs"
git -C "$T/work" add -A && git -C "$T/work" commit -qm "main reads docs/x.md" && git -C "$T/work" push -q origin HEAD:main
: > "$T/gh.log"; ff --noboot 8
check "the combined commit runs as a full batch" 'has "the combined commit needs the boot jobs: running as a full batch" && ! grep -q -- "--head batch/noboot-" "$T/gh.log" && has "  #8: MERGED"'

echo "== batch-ff.sh: a resumed no-boot batch keeps its gate"
nb_setup 8 9
export CI_SKIP_BOOT=1
touch "$T/kill-on-ready"; ff --noboot 8 9
check "the state says no-boot"                'grep -q "^noboot=1" "$T/state/current-batch" && grep -q "^name=batch/noboot-" "$T/state/current-batch"'
# With the flag wiped, only the branch name can give the lint-only gate, and
# the full gate would refuse the skipped boot jobs.
sed -i 's/^phase=.*/phase=pushed/; s/^noboot=.*/noboot=/' "$T/state/current-batch"
ff --resume
check "the branch name decides it: it lands"  'has "  #8: MERGED" && has "  #9: MERGED"'
nb_setup 1
ff 1
check "a full batch refuses skipped boot jobs" 'has "batch NOT green" && ! has "#1: MERGED"'
unset CI_SKIP_BOOT

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
# With more open PRs than a listing holds, the oldest batch-ready ones fell
# off it and were never batched (96 open PRs at --limit 80, 2026-11-19).
check "every open-PR listing asks for 1000"   'grep "pr list .*--state open" "$T/gh.log" | grep -v -- "--head" | grep -q . && ! grep "pr list .*--state open" "$T/gh.log" | grep -v -- "--head" | grep -qv -- "--limit 1000"'

# 11-24: #1188's head was on main already, carried in by another PR's merge,
# and GitHub kept it open; each loop batched it and put an empty commit on main.
carried() {  # #8, its head already on main, open
  mkpr 8 feat/carried main docs/c.md; touch "$T/prs/8/stays-open"
  git -C "$T/work" push -q origin feat/carried:main && git -C "$T/work" fetch -q origin
  git -C "$T/work" checkout -q --detach origin/main
  m=$(git -C "$T/origin.git" rev-parse main)
}
twin() {  # #8, its change already on main by another commit, its head not
  for n in 1 2 3 4 5 7; do : > "$T/prs/$n/labels"; done
  mkpr 8 feat/twin main docs/c.md
  git -C "$T/work" checkout -q --detach origin/main
  mkdir -p "$T/work/docs"; echo feat/twin > "$T/work/docs/c.md"
  git -C "$T/work" add -A && git -C "$T/work" commit -qm twin && git -C "$T/work" push -q origin HEAD:main
  m=$(git -C "$T/origin.git" rev-parse main)
}
auto_wait() {  # runs batch-auto.sh until its log has $1, then stops it
  : > "$T/gh.log"
  bash "$B/batch-auto.sh" > /dev/null & A=$!
  for i in $(seq 1 120); do grep -q "$1" "$T/state/batch-auto.log" 2>/dev/null && break; /bin/sleep 1 2>/dev/null || /usr/bin/sleep 1; done
  touch "$T/state/batch-auto.stop"; wait $A
  L="$T/state/batch-auto.log"
}

echo "== batch-ff.sh: a batch that adds nothing to main stops before its CI"
setup; carried
: > "$T/gh.log"; ff 8
check "it stops, main unchanged, no CI"       'has "the batch adds nothing to main" && ! has "waiting for CI" && ! grep -q "pr create" "$T/gh.log" && [ "$(git -C "$T/origin.git" rev-parse main)" = "$m" ]'

echo "== batch-auto.sh: a PR whose head is already on main is unlabelled, not batched"
setup; carried
for n in 2 3 4 5 7; do : > "$T/prs/$n/labels"; done
auto_wait "#1 merged"
check "its label goes, with a comment"        'grep -q "#8 left out: its head [0-9a-f]* is already on main; label removed" "$L" && [ ! -s "$T/prs/8/labels" ] && grep -q "pr comment 8 .*already on main" "$T/gh.log"'
check "it is in no batch; the rest lands"     '! grep -qE "batch: (.* )?8( |$)" "$L" && grep -q "#1 merged" "$L"'

echo "== batch-auto.sh: a landed PR GitHub still shows open loses its label, unless pushed to as it landed"
setup
touch "$T/prs/5/stays-open"; for n in 1 2 3 4 7; do : > "$T/prs/$n/labels"; done
# The lane pushes to #5 after land checked its head, once: the first landing
# keeps the label, and the second lands the new commit and removes it.
cat > "$T/on-ready" <<EOF
git -C "$T/work" checkout -q feat/d && echo more >> "$T/work/d.txt" && git -C "$T/work" commit -qam more \
  && git -C "$T/work" push -q origin feat/d && git -C "$T/work" rev-parse HEAD > "$T/moved-head"
EOF
auto_wait "#5 landed but"; rm -f "$T/on-ready"
check "pushed to: label kept, the commit lands" '[ "$(grep -c "batch: 5$" "$L")" = 2 ] && git -C "$T/origin.git" merge-base --is-ancestor "$(cat "$T/moved-head")" main'
check "then its label goes, with a comment"   '[ "$(grep -c "#5 landed but GitHub still shows it open; label removed" "$L")" = 1 ] && [ ! -s "$T/prs/5/labels" ] && grep -q "pr comment 5 .*within a few minutes" "$T/gh.log"'

echo "== batch-auto.sh: a PR whose change is already on main is unlabelled, with no CI run"
setup; twin
auto_wait "#8 left out"
check "its label goes, with a comment"        'grep -q "#8 left out: its batch adds nothing to main; label removed" "$L" && [ ! -s "$T/prs/8/labels" ] && grep -q "pr comment 8 .*already on main" "$T/gh.log"'
check "no batch PR or CI; main unchanged"     '! grep -q "pr create" "$T/gh.log" && [ "$(git -C "$T/origin.git" rev-parse main)" = "$m" ]'

echo "== batch-auto.sh: a no-op batch resumed after its PR was pushed to keeps the label"
setup; twin
# A green state saved while #8 added nothing (before compose() checked for it).
b=$m; h=$(cat "$T/prs/8/sha")
c=$(git -C "$T/work" commit-tree "$b^{tree}" -p "$b" -p "$h" -m "batch: #8 (feat/twin)")
mkdir -p "$T/state"
printf 'phase=green\nname=batch/1124-0000\nbase=%s\nc=%s\nbpr=999\nprs=8\nheads_s=%s\nbrs_s=feat/twin\n' "$b" "$c" "$h" > "$T/state/current-batch"
git -C "$T/work" checkout -q feat/twin && echo y > "$T/work/docs/d.md" && git -C "$T/work" add -A && git -C "$T/work" commit -qm more
git -C "$T/work" push -q origin feat/twin; nh=$(git -C "$T/work" rev-parse HEAD); git -C "$T/work" checkout -q --detach origin/main
auto_wait "#8 merged"
check "the resume stops on the moved head"    'grep -q "batch stopped: #8 was pushed to during the batch" "$L" && ! grep -q "adds nothing" "$L"'
check "the label stays; the new commit lands" 'grep -q "#8 merged" "$L" && git -C "$T/origin.git" merge-base --is-ancestor "$nh" main'

echo "== batch-auto.sh: a failed batch names its failing cases in the log"
setup
for n in 2 3 4 5 7; do : > "$T/prs/$n/labels"; done
printf '%s\t%s\t%s\n' "Boot test (kvm, shard 1/3)" "UNKNOWN STEP" "2026-11-18T21:07:27Z xtask: test case c-build: FAIL: QEMU exited with exit status: 35" \
  "Boot test (kvm, shard 1/3)" "UNKNOWN STEP" "2026-11-18T21:18:32Z xtask: failed test cases: c-build" > "$T/failed.log"
CI_RESULT=failure bash "$B/batch-auto.sh" > /dev/null & A=$!
for i in $(seq 1 120); do grep -q "failed alone" "$T/state/batch-auto.log" 2>/dev/null && break; /bin/sleep 1 2>/dev/null || /usr/bin/sleep 1; done
touch "$T/state/batch-auto.stop"; wait $A; rm -f "$T/failed.log"
L="$T/state/batch-auto.log"
check "the log names the failing case"        'grep -q "batch of 1 failed: .*test case c-build: FAIL" "$L"'
check "the PR failed alone, its case named"   'grep -q "#1 failed alone: .*test case c-build: FAIL" "$L" && [ ! -s "$T/prs/1/labels" ]'

auto_until() {  # runs batch-auto.sh until its log matches $1, then stops it
  # The no-boot lane off: it would land the .txt PRs ahead in a batch of their own.
  KAI_BATCH_NOBOOT=0 bash "$B/batch-auto.sh" > /dev/null & A=$!
  for i in $(seq 1 120); do grep -qE "$1" "$T/state/batch-auto.log" 2>/dev/null && break; /bin/sleep 1 2>/dev/null || /usr/bin/sleep 1; done
  touch "$T/state/batch-auto.stop"; wait $A
}
compile_log() {  # rustc errors at each FILE:LINE given, as gh run view --log-failed prints them
  # The shape of the 11-23 batch run's log: ANSI colour, under "UNKNOWN STEP".
  local at e=$'\033' j="Format, lint, host unit tests" s="UNKNOWN STEP"
  for at in "$@"; do
    printf '%s\t%s\t%s\n' \
      "$j" "$s" "2026-11-23T04:16:08.8681079Z ${e}[1m${e}[91merror[E0609]${e}[0m${e}[1m: no field \`last_use_ms\` on type \`&mut Nap\`${e}[0m" \
      "$j" "$s" "2026-11-23T04:16:08.8683125Z   ${e}[1m${e}[94m--> ${e}[0m$at:14" \
      "$j" "$s" "2026-11-23T04:16:08.8683945Z    ${e}[1m${e}[94m|${e}[0m"
  done > "$T/compile.log"
}

echo "== batch-auto.sh: a batch that does not compile goes again without the PR rustc names"
setup
for n in 2 3 4 7; do : > "$T/prs/$n/labels"; done
mkpr 8 feat/k main kainet/src/tcp.rs
git -C "$T/work" checkout -q --detach origin/main
cat "$T/prs/8/sha" > "$T/fail-with"
compile_log kainet/src/tcp.rs:212
auto_until "failed alone|keep their labels|splitting"
L="$T/state/batch-auto.log"
check "the error and its file are logged"     'grep -q "does not compile: kainet/src/tcp.rs:212:14  error\[E0609\]: no field" "$L"'
check "the rest goes again whole, no halving" 'grep -q "trying it without 8 (8 touch those files)" "$L" && ! grep -q splitting "$L"'
check "the rest lands"                        'grep -q "#1 merged" "$L" && grep -q "#5 merged" "$L"'
check "the culprit goes alone, fails, unlabelled with the error" 'grep -q "the rest landed; 8 go on as a batch of their own" "$L" && grep -q "#8 failed alone" "$L" && [ ! -s "$T/prs/8/labels" ] && grep -q "pr comment 8 .*compile error: kainet/src/tcp.rs:212:14" "$T/gh.log" && ! grep -q "#8 merged" "$L"'

echo "== batch-auto.sh: two PRs that clash only together: the rest lands, then one of them"
setup
for n in 2 3 4 7; do : > "$T/prs/$n/labels"; done
mkpr 8 feat/k main kainet/src/tcp.rs
mkpr 9 feat/k2 main kainet/src/io.rs
git -C "$T/work" checkout -q --detach origin/main
echo "$(cat "$T/prs/8/sha") $(cat "$T/prs/9/sha")" > "$T/fail-with"
compile_log kainet/src/tcp.rs:212 kainet/src/io.rs:40
auto_until "#9 failed alone|#8 failed alone"
L="$T/state/batch-auto.log"
check "both leave the batch"                  'grep -q "trying it without 8 9 (8 9 touch those files)" "$L"'
check "the rest lands, then they go together" 'grep -q "#1 merged" "$L" && grep -q "#5 merged" "$L" && grep -q "the rest landed; 8 9 go on as a batch of their own" "$L"'
check "one lands, the other fails alone"      'grep -q "#8 merged" "$L" && grep -q "#9 failed alone" "$L" && [ ! -s "$T/prs/9/labels" ]'

# The review of #1264: A (8) changes a signature, main calls it from a file
# C (9) also touches, so rustc names C's file while A is in the batch. C must
# not lose its label for the rest failing again; it lands on a run of its own.
echo "== batch-auto.sh: the rest fails again; the innocent PR rustc pointed at still lands"
setup
for n in 2 3 4 7; do : > "$T/prs/$n/labels"; done
mkpr 8 feat/sig main kabi/src/call.rs
mkpr 9 feat/caller main kernel/src/process/mod.rs
git -C "$T/work" checkout -q --detach origin/main
cat "$T/prs/8/sha" > "$T/fail-with"
compile_log kernel/src/process/mod.rs:212
: > "$T/gh.log"
auto_until "#9 (merged|failed alone|unlabelled)"
L="$T/state/batch-auto.log"
check "C leaves, the rest goes again"         'grep -q "trying it without 9 (9 touch those files)" "$L"'
check "the rest fails again and is halved"    'grep -q "splitting" "$L" && grep -q "#1 merged" "$L" && grep -q "#5 merged" "$L"'
check "A fails alone, with the error"         'grep -q "#8 failed alone: .*compile error: kernel/src/process/mod.rs:212:14" "$L" && [ ! -s "$T/prs/8/labels" ]'
check "C goes on alone and lands, uncommented" 'grep -q "the rest landed; 9 go on as a batch of their own" "$L" && grep -q "#9 merged" "$L" && ! grep -q "pr comment 9 " "$T/gh.log" && ! grep -q "pr edit 9 " "$T/gh.log"'

echo "== batch-auto.sh: a culprit inside a stack leaves with its stack"
setup
for n in 2 3 4 7; do : > "$T/prs/$n/labels"; done
mkpr 8 feat/k main kainet/src/tcp.rs
mkpr 9 feat/k2 feat/k kainet/src/io.rs
git -C "$T/work" checkout -q --detach origin/main
cat "$T/prs/9/sha" > "$T/fail-with"
compile_log kainet/src/io.rs:40
auto_until "#9 failed alone|keep their labels"
L="$T/state/batch-auto.log"
check "the whole stack leaves"                'grep -q "trying it without 8 9 (9 touch those files)" "$L"'
check "the rest lands, the stack goes on"     'grep -q "#1 merged" "$L" && grep -q "#5 merged" "$L" && grep -q "the rest landed; 8 9 go on as a batch of their own" "$L"'
check "the parent lands, the child fails"     'grep -q "#8 merged" "$L" && grep -q "#9 failed alone: .*kainet/src/io.rs:40:14" "$L" && [ ! -s "$T/prs/9/labels" ]'

echo "== batch-auto.sh: errors in files no PR touches: the usual halving"
setup
for n in 2 3 4 7; do : > "$T/prs/$n/labels"; done
mkpr 8 feat/k main kainet/src/tcp.rs
git -C "$T/work" checkout -q --detach origin/main
cat "$T/prs/8/sha" > "$T/fail-with"
compile_log kernel/src/process/mod.rs:212
auto_until "#8 failed alone|keep their labels"
L="$T/state/batch-auto.log"
check "no PR leaves first; the batch is split" '! grep -q "trying it without" "$L" && grep -q "batch of 1 5 8 failed; splitting it" "$L"'
check "the others land, the PR fails alone"   'grep -q "#1 merged" "$L" && grep -q "#5 merged" "$L" && grep -q "#8 failed alone: .*compile error: kernel/src/process/mod.rs:212:14" "$L" && [ ! -s "$T/prs/8/labels" ]'

repush() {  # $1 PR, $2 N or "fluency": at the Nth CI wait, or the fluency wait, its lane pushes a new head, a sibling of the old one
  local n=$1 br hook="$T/on-ci-wait-$2"; br=$(cat "$T/prs/$1/branch")
  [ "$2" = fluency ] && hook="$T/on-fluency-wait"
  cat >> "$hook" <<EOF
new=\$(git -C "$T/origin.git" commit-tree "refs/heads/$br^{tree}" -p "refs/heads/$br^" -m "$br fixed")
git -C "$T/origin.git" update-ref "refs/heads/$br" "\$new"
echo "\$new" > "$T/prs/$n/sha"
EOF
}
no_word_to() { ! grep -qE "pr (comment|edit) $1 " "$T/gh.log"; }  # $1 PR: never commented on or unlabelled

echo "== batch-auto.sh: none of the rest lands; the culprit keeps its label"
setup
for n in 2 3 4 7; do : > "$T/prs/$n/labels"; done
mkpr 9 feat/caller main kernel/src/process/mod.rs
git -C "$T/work" checkout -q --detach origin/main
printf '%s\n' "$(cat "$T/prs/1/sha")" "$(cat "$T/prs/5/sha")" > "$T/fail-with"
compile_log kernel/src/process/mod.rs:212
: > "$T/gh.log"
auto_until "keep their labels"
L="$T/state/batch-auto.log"
check "the rest fails, each PR alone"         'grep -q "trying it without 9" "$L" && grep -q "#1 failed alone" "$L" && grep -q "#5 failed alone" "$L"'
check "the culprit keeps its label, no word"  'grep -q "the rest did not land either; 9 keep their labels" "$L" && no_word_to 9 && ! grep -q "#9 failed alone" "$L"'

echo "== batch-auto.sh: a culprit unlabelled while the rest runs is left out"
setup
for n in 2 3 4 7; do : > "$T/prs/$n/labels"; done
mkpr 9 feat/caller main kernel/src/process/mod.rs
git -C "$T/work" checkout -q --detach origin/main
cat "$T/prs/9/sha" > "$T/fail-with"
compile_log kernel/src/process/mod.rs:212
echo ": > '$T/prs/9/labels'" > "$T/on-ci-wait-2"
: > "$T/gh.log"
auto_until "batch stopped"
L="$T/state/batch-auto.log"
check "the rest lands, the culprit goes on"   'grep -q "#1 merged" "$L" && grep -q "the rest landed; 9 go on" "$L"'
check "it is left out: no run, no word"       'grep -q "batch stopped: nothing to merge" "$L" && ! grep -qE "#9 (merged|failed)" "$L" && no_word_to 9'

echo "== batch-auto.sh: a culprit re-pushed while the rest runs goes on at its new head"
setup
for n in 2 3 4 7; do : > "$T/prs/$n/labels"; done
mkpr 9 feat/caller main kernel/src/process/mod.rs
git -C "$T/work" checkout -q --detach origin/main
cat "$T/prs/9/sha" > "$T/fail-with"
compile_log kernel/src/process/mod.rs:212
repush 9 2
: > "$T/gh.log"
auto_until "#9 (merged|failed alone)"
L="$T/state/batch-auto.log"
check "the new head lands, no word"           'grep -q "the rest landed; 9 go on" "$L" && grep -q "#9 merged" "$L" && no_word_to 9'

echo "== batch-auto.sh: every PR a culprit: the usual halving"
setup
for n in 1 2 3 4 5 7; do : > "$T/prs/$n/labels"; done
mkpr 8 feat/k main kainet/src/tcp.rs
mkpr 9 feat/k2 main kainet/src/io.rs
git -C "$T/work" checkout -q --detach origin/main
cat "$T/prs/8/sha" > "$T/fail-with"
compile_log kainet/src/tcp.rs:212 kainet/src/io.rs:40
auto_until "#8 failed alone|keep their labels"
L="$T/state/batch-auto.log"
check "nobody leaves first; it is halved"     '! grep -q "trying it without" "$L" && grep -q "batch of 8 9 failed; splitting it" "$L"'
check "the guilty one fails alone, the other lands" 'grep -q "#8 failed alone" "$L" && grep -q "#9 merged" "$L" && [ ! -s "$T/prs/8/labels" ]'

echo "== batch-auto.sh: a PR re-pushed during its solo run keeps its label"
setup
for n in 2 3 4 7; do : > "$T/prs/$n/labels"; done
mkpr 9 feat/caller main kernel/src/process/mod.rs
git -C "$T/work" checkout -q --detach origin/main
cat "$T/prs/9/sha" > "$T/fail-with"
compile_log kernel/src/process/mod.rs:212
repush 9 3
: > "$T/gh.log"
auto_until "#9 (merged|failed alone: )"
L="$T/state/batch-auto.log"
check "it fails alone on the old head"        'grep -q "#9 failed alone, but it was pushed to since the batch took it; label kept" "$L"'
check "no comment, no unlabel"                'no_word_to 9'
check "the new head lands next"               'grep -q "#9 merged" "$L"'

echo "== batch-auto.sh: a model PR re-pushed during the fluency wait keeps its label"
setup
printf 'fluency: 40/50 first try (80%%), 42/50 after one repair (84%%)\nthe fluency gate fails\n' > "$T/fluency.log"
repush 2 fluency
# The lane's fix works: the fluency run after this one passes.
printf 'fluency: 48/50 first try (96%%), 49/50 after one repair (98%%)\nthe fluency gate passes\n' > "$T/fluency-pass"
echo "echo \"cp '$T/fluency-pass' '$T/fluency.log'\" > '$T/on-fluency-wait'" >> "$T/on-fluency-wait"
: > "$T/gh.log"
auto_until "#2 (merged|failed the fluency gate:|failed alone)"
L="$T/state/batch-auto.log"
check "it fails the gate on the old head"     'grep -q "#2 failed the fluency gate, but it was pushed to since the batch took it; label kept" "$L"'
check "no comment, no unlabel"                'no_word_to 2'
check "the rest lands, then the new head"     'grep -q "#1 merged" "$L" && grep -q "#2 merged" "$L"'

echo "== batch-ff.sh: each rustc error is paired with its own --> line"
setup
cat "$T/prs/1/sha" > "$T/fail-with"
e=$'\033'
{ for l in "${e}[1m${e}[91merror[E0308]${e}[0m${e}[1m: mismatched types${e}[0m" \
           "  ${e}[1m${e}[94m--> ${e}[0mkainet/src/plain.rs:10:5" \
           "" \
           "${e}[1m${e}[91merror[E0277]${e}[0m${e}[1m: the trait bound \`Foo: Bar\` is not" \
           "satisfied${e}[0m" \
           "  ${e}[1m${e}[94m--> ${e}[0mkainet/src/wrapped.rs:20:9" \
           "" \
           "${e}[1m${e}[91merror[E0425]${e}[0m${e}[1m: cannot find value \`x\` in this scope${e}[0m" \
           "   ${e}[1m${e}[94m= ${e}[0m${e}[1mnote${e}[0m: a note before the span" \
           "  ${e}[1m${e}[94m--> ${e}[0mkainet/src/noted.rs:30:1" \
           "${e}[1m${e}[91merror[E0599]${e}[0m${e}[1m: no method named \`go\`${e}[0m" \
           "" \
           "  ${e}[1m${e}[94m--> ${e}[0mpast/blank.rs:1:1" \
           "${e}[1m${e}[91merror${e}[0m: could not compile \`kainet\` (lib) due to 4 previous errors" \
           "  ${e}[1m${e}[94m--> ${e}[0mstray/after.rs:2:2"; do
    printf '%s\t%s\t%s\n' "Format, lint, host unit tests" "UNKNOWN STEP" "2026-11-23T04:16:08.8681079Z $l"
  done; } > "$T/compile.log"
ff 1
check "the plain shape"                       'has "    compile error: kainet/src/plain.rs:10:5  error[E0308]: mismatched types"'
check "a header that wraps"                   'has "    compile error: kainet/src/wrapped.rs:20:9  error[E0277]: the trait bound \`Foo: Bar\` is not"'
check "a note between header and -->"         'has "    compile error: kainet/src/noted.rs:30:1  error[E0425]: cannot find value \`x\` in this scope"'
check "a blank line ends the wait"            '! has "past/blank.rs"'
check "could not compile pairs with nothing"  '! has "stray/after.rs" && [ "$(grep -c "compile error: " "$T/out")" = 3 ]'

echo "== batch-auto.sh: a modify/delete conflict names its file"
setup
for n in 1 2 3 4 5 7; do : > "$T/prs/$n/labels"; done
mkpr 9 feat/z main base.txt
git -C "$T/work" checkout -q -B gone origin/main && git -C "$T/work" rm -q base.txt && git -C "$T/work" commit -qm "rm base" && git -C "$T/work" push -q origin HEAD:main
git -C "$T/work" fetch -q origin && git -C "$T/work" checkout -q --detach origin/main
auto_until "left out: conflicts in"
L="$T/state/batch-auto.log"
check "the deleted file is listed"            'grep -q "#9 left out: conflicts in base.txt" "$L" && [ ! -s "$T/prs/9/labels" ]'

echo "== batch-ff.sh: an error with no span is not credited to a later note's file"
setup
cat "$T/prs/1/sha" > "$T/fail-with"
e=$'\033'
{ for l in "${e}[1m${e}[91merror[E0599]${e}[0m${e}[1m: no method named \`go\`${e}[0m" \
           "${e}[1m${e}[96mnote${e}[0m: the method is defined here" \
           "  ${e}[1m${e}[94m--> ${e}[0mother/note.rs:5:1" \
           "" \
           "${e}[1m${e}[91merror[E0432]${e}[0m${e}[1m: unresolved import \`foo\`${e}[0m" \
           "${e}[1m${e}[96mhelp${e}[0m: a similar path exists" \
           "  ${e}[1m${e}[94m--> ${e}[0mother/help.rs:6:1" \
           "" \
           "${e}[1m${e}[91merror[E0001]${e}[0m${e}[1m: a build script failed${e}[0m" \
           "Caused by:" \
           "  ${e}[1m${e}[94m--> ${e}[0mother/caused.rs:7:1" \
           "" \
           "${e}[1m${e}[91merror[E0308]${e}[0m${e}[1m: mismatched types${e}[0m" \
           "  ${e}[1m${e}[94m--> ${e}[0mkainet/src/real.rs:9:9"; do
    printf '%s\t%s\t%s\n' "Format, lint, host unit tests" "UNKNOWN STEP" "2026-11-23T04:16:08.8681079Z $l"
  done; } > "$T/compile.log"
ff 1
check "a column-0 note, help or Caused by ends the wait" '! has "other/note.rs" && ! has "other/help.rs" && ! has "other/caused.rs"'
check "an error with its own span still counts" 'has "    compile error: kainet/src/real.rs:9:9  error[E0308]: mismatched types" && [ "$(grep -c "compile error: " "$T/out")" = 1 ]'

echo "== batch-ff.sh: finish compares the branch on origin, not headRefOid, which lags a push"
setup
touch "$T/prs/1/stays-open"
# After land's head checks, GitHub still answers with another head (a lag).
echo "git -C \"$T/work\" rev-parse HEAD > \"$T/prs/1/lagging-head\"" > "$T/on-ready"
ff 1
check "landed but still open is told despite the lag" 'has "#1 landed but GitHub still shows it open"'

echo "== batch-ff.sh: a PR pushed to as it lands is not told as landed, though headRefOid still shows the old head"
setup
touch "$T/prs/1/stays-open"
old=$(git -C "$T/origin.git" rev-parse feat/a)
cat > "$T/on-ready" <<EOF
echo $old > "$T/prs/1/lagging-head"
git -C "$T/work" checkout -q feat/a && echo more >> "$T/work/a.txt" && git -C "$T/work" commit -qam more && git -C "$T/work" push -q origin feat/a
EOF
ff 1
check "no landed-but-open line for a moved branch" '! has "#1 landed but GitHub still shows it open"'

echo "== batch-ff.sh: the files split out of agentcore.rs count as the model's, as agentcore.rs does"
setup
mkpr 8 feat/ac1 main shellcore/src/agentcore/turn.rs
mkpr 9 feat/ac2 main shellcore/src/agentcore.rs
mkpr 10 feat/other main shellcore/src/other.rs
git -C "$T/work" checkout -q --detach origin/main
ff 8 9 10
check "an agentcore/ file is a model change"   'has "  + #8 (feat/ac1)" && has "  - #9 changes what the model sees, and so does #8"'
check "a neighbouring shellcore file is not"  'has "  + #10 (feat/other)"'

echo "== config.sh: MODEL_PATHS names every file that feeds a model prompt"
setup
mp=$(env -u KAI_MODEL_PATHS bash -c 'source "$1/config.sh" && echo "$MODEL_PATHS"' _ "$B")
for f in llmproto/src/act.rs infer/src/lib.rs quant/src/lib.rs gguf/src/lib.rs user/src/bin/llm.rs user/src/bin/llm/local.rs \
  user/src/pool.rs agentabi/src/lib.rs shellcore/src/agentcore.rs shellcore/src/agentcore/turn.rs shellcore/src/state.rs \
  goalcore/src/planner.rs user/src/bin/goals.rs user/src/bin/shell/ui.rs \
  shellcore/src/ui.rs shellcore/src/apps.rs shellcore/src/apps/rows.rs shellcore/src/appbuild.rs shellcore/src/request.rs \
  shellcore/src/guard.rs shellcore/src/tune.rs shellcore/src/cmode/program.rs \
  shellcore/src/script.rs shellcore/src/script/known.rs shellcore/src/script/net.rs shellcore/src/actions.rs shellcore/src/answers.rs shellcore/src/engine.rs shellcore/src/eval.rs livecore/src/why.rs \
  deskcore/src/agent.rs deskcore/src/view/mod.rs; do
  check "$f is a model path" "echo '$f' | grep -qE '$mp'"
done
for f in shellcore/src/other.rs shellcore/src/cmode/saved.rs shellcore/src/uix.rs goalcore/src/goal.rs goalcore/src/planner/tests.rs \
  user/src/bin/shell/host.rs user/src/bin/desktop/agent.rs deskcore/src/view/tests.rs deskcore/src/agent_tests.rs deskcore/src/files.rs \
  shellcore/src/script/look.rs shellcore/src/script/bind.rs shellcore/src/script/stub.rs livecore/src/ledger.rs; do
  check "$f is not" "! (echo '$f' | grep -qE '$mp')"
done

echo "== setup(): no stub hook is left behind for the next case"
for f in ready-hook fluency-hook on-ready moved-head edit-fails; do : > "$T/$f"; done
setup
check "hook files are cleared"                '(for f in ready-hook fluency-hook on-ready moved-head edit-fails; do [ -e "$T/$f" ] && exit 1; done; exit 0)'

echo "== batch-auto.sh: unlabel that cannot remove the label posts no comment and says so"
setup; carried
for n in 2 3 4 5 7; do : > "$T/prs/$n/labels"; done
: > "$T/edit-fails"
auto_wait "removing batch-ready failed"
check "the failure is logged"                 'grep -q "#8: removing batch-ready failed; no comment posted" "$L"'
check "no comment says the label is gone"     '! grep -q "pr comment 8 " "$T/gh.log"'
check "the log does not claim it was removed" '! grep -q "#8 left out: .*label removed" "$L"'
rm -f "$T/edit-fails"

echo "branches left: $(branches)"
[ $fails = 0 ] && echo "all batcher tests pass" || { echo "$fails batcher test(s) failed"; exit 1; }
