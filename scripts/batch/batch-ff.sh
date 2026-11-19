#!/usr/bin/env bash
# Merges a batch of ready PRs at once, under the founder's 2026-10-26 OK for
# batched merging:
#   1. Combines the PRs, in the order given, on one test branch made from
#      current main with `git merge-tree` (no checkout), stopping on any conflict.
#      At most one PR that changes what the model sees (MODEL_PATHS) goes in a
#      batch; a second one is left out, keeps its label, and goes in the next.
#   2. Opens a draft "batch CI" PR for that branch so ci.yml runs once on the
#      combination, and requires lint, the 6 boot shards and the 3 observe jobs
#      green.
#   3. If a PR touches what the model sees, runs the fluency gate on the batch
#      head and requires it to pass at >= FLUENCY_MIN% after repair.
#   4. Fast-forwards main to the tested batch commit itself, provided main has
#      not moved and no PR's head changed during the run. Each PR's head is then
#      in main, so GitHub marks it merged; stacked PRs are retargeted to main
#      first. What lands on main is exactly the tree CI tested.
#   5. Deletes each landed PR's head branch (see delete_heads).
# Combining honours .gitattributes.
# A failed batch merges nothing: fall back to merge-chain.sh, one at a time.
#
# No-boot: `batch-ff.sh --noboot <PRs>` takes only PRs that noboot.sh finds
# change nothing the boot jobs see, on a `batch/noboot-...` branch. ci.yml runs
# no boot job for it, and its lint job fails it if noboot.sh disagrees; it lands
# on lint and the other workflows alone (2026-11-19, to spend fewer runners).
#
# Resume: from the push of the batch branch on, the batch is recorded in
# $S/current-batch. If the PC stops, `batch-ff.sh --resume` (batch-auto.sh runs
# it first thing) picks the batch up: it waits for CI if CI had not passed yet,
# and lands it only if main and every PR head are still what was tested.
# Settings: config.sh.
. "$(dirname "$0")/config.sh"
STATE="$S/current-batch"

field() { gh pr view "$1" --repo "$R" --json "$2" --jq ".$2"; }

# Three hours: on 11-10 a full runner queue took a batch with nothing red past
# the old 90-minute cap, and the batcher split it as if it had failed.
wait_ci() {  # $1 branch, $2 sha -> prints run id when completed
  for i in $(seq 1 180); do
    c=$(gh run list --repo "$R" --branch "$1" --workflow ci.yml --limit 8 --json headSha,status,databaseId \
        --jq ".[] | select(.headSha==\"$2\") | (.databaseId|tostring)+\" \"+.status" | head -1)
    case "$c" in *completed) echo "${c%% *}"; return 0;; esac
    sleep 60
  done
  return 1
}

gates_green() {  # $1 run id
  # A GitHub 5xx returns nothing; retry, so a green batch isn't read as red
  # and split (10-30: a 502 split a fully green batch of #473 and #460).
  local try
  for try in 1 2 3 4; do
    g=$(gh run view "$1" --repo "$R" --json jobs --jq '.jobs[] | select(.name|test("^(Format, lint|Boot test|Observe determinism)")) | .name+"="+(.conclusion//.status)' 2>/dev/null)
    [ -n "$g" ] && break
    sleep 30
  done
  echo "$g" | sed 's/^/    /'
  # A no-boot batch runs lint alone (ci.yml skips the boot jobs for it, after
  # its lint job checked that nothing it changes reaches them).
  if [ -n "$noboot" ]; then
    echo "$g" | grep -q "^Format, lint[^=]*=success$" && ! echo "$g" | grep -qvE "=(success|skipped)$"
    return
  fi
  [ "$(echo "$g" | grep -c "=")" -ge 10 ] && [ "$(echo "$g" | grep -vc "=success$")" = 0 ]
}

# Observe determinism flips on plain main about 1 boot in 5 (lane K, 2026-10-27),
# so a run whose only red gates are observe jobs gets one re-run, not a split.
# TCG boot shards get the same single re-run (timing deadlines, e.g. c-prompt 10-30).
# So does a KVM shard GitHub cancelled (the job time limit, #591 on 11-03) or one
# that failed before its boot step ran (the QEMU cache download timed out, #592).
observe_only() {  # $1 run id
  bad=$(gh run view "$1" --repo "$R" --json jobs --jq '.jobs[] | select(.name|test("^(Format, lint|Boot test|Observe determinism)")) | select(.conclusion!="success") | .name+"|"+(.conclusion//.status)+"|"+([.steps[]|select(.name|test("^Boot kernel"))|.conclusion][0]//"none")')
  [ -n "$bad" ] && ! echo "$bad" | grep -vE "^(Observe determinism|Boot test \(tcg)" | grep -vE "^Boot test \(kvm.*(\|cancelled\||\|skipped$)" | grep -q .
}

# The path-filtered workflows (kaifs, kaituf, pkgrepo, Release, DCO, Token
# counts) run on the batch head too: every one must finish without a failure.
# #256 merged on 2026-10-27 with pkgrepo red, since only ci.yml was checked.
others_ok() {  # $1 branch, $2 sha
  for i in $(seq 1 60); do
    o=$(gh run list --repo "$R" --branch "$1" --limit 30 --json headSha,workflowName,status,conclusion \
        --jq ".[] | select(.headSha==\"$2\" and .workflowName!=\"CI\" and (.workflowName|test(\"Fluency\")|not)) | .workflowName+\"=\"+(.conclusion//.status)")
    echo "$o" | grep -qvE "=(success|skipped|neutral|failure|cancelled|timed_out|action_required)$" || break
    sleep 60
  done
  echo "$o" | sed 's/^/    /'
  ! echo "$o" | grep -qE "=(failure|cancelled|timed_out|action_required)$"
}

fluency_ok() {  # $1 branch, $2 sha; dispatches the run unless $fluency is set (a resume)
  if [ -z "$fluency" ]; then
    gh workflow run fluency-model.yml --repo "$R" --ref "$1" >/dev/null || return 1
    fluency=dispatched; save_state pushed
    sleep 45
  fi
  for i in $(seq 1 150); do
    f=$(gh run list --repo "$R" --workflow fluency-model.yml --branch "$1" --limit 3 --json headSha,status,databaseId \
        --jq ".[] | select(.headSha==\"$2\") | (.databaseId|tostring)+\" \"+.status" | head -1)
    case "$f" in *completed) break;; esac
    sleep 60
  done
  case "$f" in *completed) ;; *) echo "    the fluency run did not finish"; return 2;; esac
  id=${f%% *}
  local log="$S/fluency-$id.log"
  # The log can be incomplete right after the run ends: read until the verdict is in.
  for i in 1 2 3 4 5 6; do
    gh run view "$id" --repo "$R" --log > "$log" 2>/dev/null
    grep -qE "the fluency gate (passes|fails)" "$log" && break
    sleep 30
  done
  line=$(grep -oE "fluency: [0-9]+/50 first try \([0-9]+%\), [0-9]+/50 after one repair \([0-9]+%\)" "$log" | head -1)
  echo "    fluency run $id: $line"
  pct=$(echo "$line" | sed -nE 's/.*after one repair \(([0-9]+)%\).*/\1/p')
  [ -n "$pct" ] && [ "$pct" -ge "$FLUENCY_MIN" ] && grep -q "the fluency gate passes" "$log"
}

cleanup() {  # $1 batch branch, $2 batch PR number
  # Only a batch branch: `--branch ""` would list, and cancel, the newest runs
  # on every branch, and `push --delete ""` is no better.
  case "$1" in batch/?*) ;; *) return 0;; esac
  # Closing a PR leaves its queued runs in the queue: cancel them first.
  for id in $(gh run list --repo "$R" --branch "$1" --limit 10 --json databaseId,status \
              --jq '.[] | select(.status!="completed") | .databaseId'); do
    gh run cancel "$id" --repo "$R" >/dev/null 2>&1
  done
  [ -n "$2" ] && gh pr close "$2" --repo "$R" --comment "Batch CI done; closing (not for merge)." >/dev/null 2>&1
  git -C "$G" push -q origin --delete "$1" >/dev/null 2>&1
}

# The batch so far, for a resume. Written whole and renamed, so a stop mid-write
# leaves the previous version.
save_state() {  # $1 phase: pushed (CI not yet passed) or green
  {
    printf 'phase=%q\n' "$1"
    printf 'name=%q\nbase=%q\nc=%q\nbpr=%q\n' "$name" "$base" "$c" "$bpr"
    printf 'model=%q\nrerun=%q\nfluency=%q\n' "$model" "$rerun" "$fluency"
    printf 'prs=%q\nheads_s=%q\nbrs_s=%q\n' "${batch[*]}" "${heads[*]}" "${brs[*]}"
    printf 'noboot=%q\n' "$noboot"
  } > "$STATE.tmp" && mv -f "$STATE.tmp" "$STATE"
}

# Ends a batch that will not land: nothing is merged, and there is nothing left
# to resume.
stop() {  # $1 message
  echo "  $1"
  rm -f "$STATE"
  cleanup "$name" "$bpr"
  exit 0
}

# Deletes the head branch of each landed PR. Kept: main and the landing branch,
# release/*, parked/*, a fork's branch (the name is not ours), a branch that is
# the base or head of any open PR, and a branch that moved since it was tested
# (the delete is leased on the tested head).
delete_heads() {  # PR numbers, index-aligned with heads[] and brs[]
  [ "$DELETE_BRANCHES" = 1 ] || return 0
  local open i=0 p br head info
  # A failed listing must not read as "no open PRs": then no branch is deleted.
  open=$(gh pr list --repo "$R" --state open --limit 1000 --json baseRefName,headRefName \
         --jq '.[] | .baseRefName, .headRefName') || { echo "  could not list open PRs; no head branch deleted"; return 0; }
  for p in "$@"; do
    br=${brs[$i]}; head=${heads[$i]}; i=$((i+1))
    case "$br" in ""|main|"$MAIN"|release/*|parked/*) echo "  kept $br (protected)"; continue;; esac
    if grep -qxF "$br" <<< "$open"; then echo "  kept $br (an open PR's base or head)"; continue; fi
    info=$(gh pr view "$p" --repo "$R" --json state,isCrossRepository --jq '.state+" "+(.isCrossRepository|tostring)')
    [ "$info" = "MERGED false" ] || { echo "  kept $br (#$p: ${info:-no answer})"; continue; }
    if git -C "$G" push -q --force-with-lease="refs/heads/$br:$head" origin ":refs/heads/$br" >/dev/null 2>&1; then
      echo "  deleted $br"
    else
      echo "  kept $br (moved since it was tested, or already gone)"
    fi
  done
}

# Waits for GitHub to mark each PR merged, then tidies up. Safe to run twice.
finish() {  # PR numbers
  for p in "$@"; do
    for k in $(seq 1 12); do
      [ "$(field $p state)" = MERGED ] && break
      sleep 10
    done
    echo "  #$p: $(field $p state)"
  done
  cleanup "$name" "$bpr"
  delete_heads "$@"
  rm -f "$STATE"
  echo "batch done"
}

# The boot cases a run failed, from its failed jobs' logs: each shard's
# "xtask: failed test cases: ..." and "xtask: test case NAME: FAIL: ..." lines,
# so batch-auto.log names the cause without opening a job's log.
failed_cases() {  # $1 run id
  gh run view "$1" --repo "$R" --log-failed 2>/dev/null \
    | grep -oE "xtask: (failed test cases: .*|test case .*: FAIL.*)" \
    | sort -u | head -20 | sed 's/^/    failed case: /'
}

verify() {
  echo "  batch PR #$bpr, head ${c:0:7}, waiting for CI"
  id=$(wait_ci "$name" "$c") || stop "CI did not finish; STOPPING"
  echo "  run $id"
  if ! gates_green "$id" && observe_only "$id"; then
    if [ -n "$rerun" ]; then
      echo "  the known-flake re-run was used before a resume; not re-running again"
    else
      echo "  only known flakes failed (observe, tcg, a cancelled or never-booted kvm shard): re-running the failed jobs once"
      rerun=1; save_state pushed
      gh run rerun "$id" --repo "$R" --failed >/dev/null 2>&1
      sleep 90
      id=$(wait_ci "$name" "$c") || stop "CI did not finish; STOPPING"
      echo "  re-run $id"
    fi
  fi
  gates_green "$id" || { failed_cases "$id"; stop "batch NOT green; STOPPING (fall back to merge-chain.sh)"; }
  others_ok "$name" "$c" || stop "batch NOT green: another workflow failed; STOPPING"
  if [ -n "$model" ]; then
    echo "  changes what the model sees ($model): running the fluency gate"
    fluency_ok "$name" "$c"; rc=$?
    [ $rc = 2 ] && stop "CI did not finish (the fluency run); STOPPING"
    [ $rc = 0 ] || stop "fluency below ${FLUENCY_MIN}% or not passing; STOPPING"
  fi
  save_state green
}

land() {  # PR numbers
  git -C "$G" fetch -q origin "$MAIN"
  [ "$(git -C "$G" rev-parse "origin/$MAIN")" = "$base" ] || stop "main moved during the batch; STOPPING"
  local i=0
  for p in "$@"; do
    [ "$(field $p headRefOid)" = "${heads[$i]}" ] || stop "#$p was pushed to during the batch; STOPPING"
    # A resume may come hours later: a PR closed or unlabelled since does not land.
    [ "$(field $p state)" = OPEN ] || stop "#$p is no longer open; STOPPING"
    gh pr view "$p" --repo "$R" --json labels --jq '.labels[].name' | grep -qx batch-ready \
      || stop "#$p is no longer batch-ready; STOPPING"
    i=$((i+1))
  done
  for p in "$@"; do
    pbase=$(field $p baseRefName)
    if [ "$pbase" != "$MAIN" ]; then
      gh pr edit $p --repo "$R" --base "$MAIN" >/dev/null && echo "  retargeted #$p from $pbase to $MAIN"
    fi
    gh pr ready $p --repo "$R" >/dev/null 2>&1
  done
  # A plain push only fast-forwards: it is refused if main moved.
  git -C "$G" push -q origin "$c:refs/heads/$MAIN" || stop "fast-forward of main refused; STOPPING"
  echo "  main fast-forwarded to the tested batch ${c:0:7}"
  finish "$@"
}

# Picks up the batch in $STATE after a stop. Prints the batch's PRs as "+ #N"
# lines, as a fresh run does, so batch-auto.sh reads the outcome the same way.
resume() {
  name=""; bpr=""
  [ -f "$STATE" ] || { echo "  no batch to resume; nothing to merge; STOPPING"; exit 0; }
  . "$STATE"
  # Checked before anything acts on them: stop's cleanup takes `name`.
  case "$name" in batch/?*) ;; *) name=""; stop "the resume state names no batch branch; STOPPING";; esac
  [[ "$c" =~ ^[0-9a-f]{40}$ && "$base" =~ ^[0-9a-f]{40}$ ]] || stop "the resume state is damaged; STOPPING"
  read -ra batch <<< "$prs"; read -ra heads <<< "$heads_s"; read -ra brs <<< "$brs_s"
  echo "== resuming batch $name ($phase) on main ${base:0:7}: ${batch[*]}"
  local i=0
  for p in "${batch[@]}"; do echo "  + #$p (${brs[$i]})"; i=$((i+1)); done
  [ ${#batch[@]} -gt 0 ] && [ ${#heads[@]} = ${#batch[@]} ] || stop "the resume state is damaged; STOPPING"
  set -- "${batch[@]}"
  git -C "$G" fetch -q origin "$MAIN"
  # Stopped after the fast-forward: only the tidying is left.
  if git -C "$G" merge-base --is-ancestor "$c" "origin/$MAIN" 2>/dev/null; then
    echo "  main already fast-forwarded to the tested batch ${c:0:7}"
    finish "$@"; exit 0
  fi
  if [ "$phase" != green ]; then
    [ "$(git -C "$G" ls-remote origin "refs/heads/$name" | cut -f1)" = "$c" ] || stop "the batch branch $name is gone; STOPPING"
    [ -n "$bpr" ] || bpr=$(gh pr list --repo "$R" --head "$name" --state open --json number --jq '.[0].number // empty' 2>/dev/null)
    [ -n "$bpr" ] || stop "batch PR could not be created; STOPPING"
    verify
  fi
  land "$@"
  exit 0
}

# A fresh batch: the PRs combined on main, the batch branch pushed and its
# draft PR open, recorded for a resume.
compose() {
  git -C "$G" fetch -q origin "$MAIN"
  base=$(git -C "$G" rev-parse "origin/$MAIN")
  name="batch/${noboot:+noboot-}$(date +%m%d-%H%M)"
  c=$base
  bpr=""
  skipped=" "
  model_heads=""
  echo "== batch $name on main ${base:0:7}: $*"
  for p in "$@"; do
    [ "$(field $p state)" = OPEN ] || { echo "  - #$p is not open; left out"; continue; }
    # A split re-runs halves from a list taken earlier; drop a PR unlabelled
    # since (11-05: #696 was unlabelled for a failing pkgrepo check mid-split).
    gh pr view "$p" --repo "$R" --json labels --jq '.labels[].name' | grep -qx batch-ready \
      || { echo "  - #$p is no longer batch-ready; left out"; continue; }
    br=$(field $p headRefName)
    pbase=$(field $p baseRefName)
    case "$skipped" in *" $pbase "*) echo "  - #$p is stacked on a left-out PR; left out"; skipped="$skipped$br "; continue;; esac
    git -C "$G" fetch -q origin "$br"
    head=$(git -C "$G" rev-parse "origin/$br")
    # A no-boot batch takes only a PR whose whole change from main is one the
    # boot jobs cannot see (noboot.sh); the rest keep their labels for a full one.
    if [ -n "$noboot" ] && ! (cd "$G" && bash "$KAI_SCRIPTS/noboot.sh" "origin/$MAIN" "$head" >/dev/null); then
      echo "  - #$p changes what the boot jobs check; left out of a no-boot batch"; skipped="$skipped$br "; continue
    fi
    # A PR that carries a model PR left out of this batch would land it unchecked.
    for mh in $model_heads; do
      if git -C "$G" merge-base --is-ancestor "$mh" "$head" 2>/dev/null; then
        echo "  - #$p contains a model PR left out of this batch; left out"; skipped="$skipped$br "; continue 2
      fi
    done
    # One PR that changes what the model sees per batch, so a fluency miss names
    # its PR without a split.
    if gh pr diff $p --repo "$R" --name-only | grep -qE "$MODEL_PATHS"; then
      if [ -n "$model" ]; then
        echo "  - #$p changes what the model sees, and so does$model; left out until the next batch"
        skipped="$skipped$br "; model_heads="$model_heads $head"
        continue
      fi
      is_model=1
    else
      is_model=""
    fi
    out=$(git -C "$G" -c attr.tree="$c" merge-tree --write-tree "$c" "$head")
    if [ $? -ne 0 ]; then
      echo "  - #$p conflicts with main or the batch so far; left out (needs a hand rebase):"
      echo "$out" | grep -E "^CONFLICT" | sed 's/^/      /'
      skipped="$skipped$br "
      continue
    fi
    t=$(echo "$out" | head -1)
    c=$(git -C "$G" commit-tree "$t" -p "$c" -p "$head" -m "batch: #$p ($br)")
    [ -n "$is_model" ] && model="$model #$p"
    batch+=("$p")
    heads+=("$head")
    brs+=("$br")
    echo "  + #$p ($br)"
  done
  [ ${#batch[@]} -gt 0 ] || { echo "  nothing to merge; STOPPING"; exit 0; }
  set -- "${batch[@]}"

  git -C "$G" push -q origin "$c:refs/heads/$name" || { echo "  push failed; STOPPING"; exit 0; }
  save_state pushed
  # CI runs only on the PR, so a failed create (GitHub 5xx, 10-30) must not leave
  # wait_ci polling for three hours: retry, then stop.
  for try in 1 2 3; do
    bpr=$(gh pr create --repo "$R" --draft --base "$MAIN" --head "$name" \
          --title "batch CI: $* (not for merge)" \
          --body "CI for a merge batch: the listed PRs combined on current main. It is closed once the batch is merged or fails." \
          | grep -oE '[0-9]+$')
    [ -n "$bpr" ] && break
    bpr=$(gh pr list --repo "$R" --head "$name" --state open --json number --jq '.[0].number // empty' 2>/dev/null)
    [ -n "$bpr" ] && break
    sleep 60
  done
  [ -n "$bpr" ] || stop "batch PR could not be created; STOPPING"
  save_state pushed
}

# Everything runs from here, once bash has read the whole file: a pull that
# rewrites this script while a batch waits on CI cannot change what runs.
main() {
  # batch-auto.sh waits while this names a live process.
  echo $$ > "$S/batch-ff.pid"
  trap 'rm -f "$S/batch-ff.pid"' EXIT
  batch=(); heads=(); brs=()
  model=""; rerun=""; fluency=""; noboot=""
  [ "$1" = --resume ] && resume
  # `--noboot`: a batch of PRs the boot jobs cannot see, gated on lint alone.
  [ "$1" = --noboot ] && { noboot=1; shift; }
  compose "$@"
  set -- "${batch[@]}"
  verify
  land "$@"
}

main "$@"; exit
