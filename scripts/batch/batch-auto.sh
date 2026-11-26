#!/usr/bin/env bash
# Runs merge batches on its own, under the founder's 2026-10-26 OK for batched
# merging: whenever no batch is running, it takes the open PRs labelled
# `batch-ready` (oldest first, at most batch-max) and runs batch-ff.sh on them.
# - A batch left unfinished by a stop (PC sleep, crash) is resumed first:
#   batch-ff.sh --resume lands it if its CI passed and nothing moved since.
# - A PR is left out (and keeps its label) while it is stacked on, or contains,
#   another open PR that is not in the batch, so nothing unlabelled lands with it.
# - A PR that conflicts with main loses its label and gets a comment saying so.
# - A batch that fails CI or fluency is split in half and each half retried, so
#   the good PRs still land; a PR that fails alone loses its label, with a comment.
# - A batch whose wait for CI runs out keeps its labels and is tried again (the
#   runner queue was full, the PRs are not at fault); two in a row stop the batcher.
# Every outcome is appended to $S/batch-auto.log. Create $S/batch-auto.stop to
# end it. Settings: config.sh.
. "$(dirname "$0")/config.sh"
LOG="$S/batch-auto.log"

say() { echo "$(date -u '+%m-%d %H:%M') $*" >> "$LOG"; }

unlabel() {  # $1 PR, $2 comment; fails only when the label could not be removed
  # A label that stays on would be batched again, with a comment that says it
  # was removed: no comment, and the log says so. Such a PR is also kept out of
  # the batches (unlabel-failed: PR and head) until it is pushed to, or the
  # loop would pick it again and fail the same way forever.
  local h
  gh pr edit "$1" --repo "$R" --remove-label batch-ready >/dev/null 2>&1 \
    || {
      h=$(gh pr view "$1" --repo "$R" --json headRefOid --jq .headRefOid 2>/dev/null)
      echo "$1 ${h:-unknown}" >> "$S/unlabel-failed"
      say "#$1: removing batch-ready failed; no comment posted; left out until it is pushed to"
      return 1
    }
  # The label is gone whatever the comment does: this status is the label's.
  gh pr comment "$1" --repo "$R" --body "$2" >/dev/null 2>&1 \
    || say "#$1: batch-ready removed, but the comment could not be posted"
  return 0
}

# Whether PR $1's head is no longer the one the batch in batch-ff.sh output $2
# tested: a lane may push a fix while CI or the fluency gate runs, and
# batch-ff.sh stops on red before it checks the heads. Such a PR keeps its
# label and the next batch tries the new head. A head that cannot be read
# counts as not moved, so the PR is unlabelled as before.
moved() {  # $1 PR, $2 batch-ff.sh output
  local tested now
  tested=$(grep -oE "^  \+ #$1 \(.*\) at [0-9a-f]{40}$" "$2" | grep -oE "[0-9a-f]{40}$")
  now=$(gh pr view $1 --repo "$R" --json headRefOid --jq .headRefOid)
  [ -n "$tested" ] && [ -n "$now" ] && [ "$now" != "$tested" ]
}

# The file each `git merge-tree` CONFLICT line names: a content conflict's
# "Merge conflict in F", and the first path of any other kind, such as a
# modify/delete's "F deleted in X and modified in Y" (#1199 on 11-23 listed
# none for kernel/src/selftest.rs and xtask/src/mock_llm.rs).
conflict_files() {  # merge-tree CONFLICT lines on stdin
  sed -nE '/CONFLICT \(/ {
    s/.*CONFLICT \([^)]*\): //
    s/^(Merge conflict in |directory in the way of |file in the way of )//
    s/^(Merge conflict in )?([^ ]+)( .*)?$/\2/
    s/,$//
    p
  }'
}

# A batch that does not compile (batch-ff.sh's "compile error: FILE:LINE"
# lines): the PRs that touch the files rustc names leave it, with their
# stacks, and the rest go again whole, rather than halving it. If any of the
# rest lands, the culprits go on as a batch of their own: touching a file
# rustc named is no proof (on 11-20 and 11-23 the error was in main's code,
# not the guilty PR's), so a culprit loses its label only by failing on its
# own run, and an innocent one lands. If none of the rest lands, the culprits
# keep their labels for the next batch. Each retry gets a strict subset, so
# this ends. Returns 1, for the usual split, when no PR, or every PR, touches
# them.
culprits_out() {  # $1 batch-ff.sh output, then the batch's PR numbers
  local out=$1 p br g hit files errs merged=""
  local -a culprits=() rest=() leaving=()
  shift
  files=$(grep "^    compile error: " "$out" | sed -E 's/^    compile error: ([^: ]+):[0-9].*/\1/' | sort -u)
  [ -n "$files" ] || return 1
  git -C "$G" fetch -q origin
  for p in "$@"; do
    br=$(gh pr view $p --repo "$R" --json headRefName --jq .headRefName)
    [ -n "$br" ] && git -C "$G" diff --name-only "origin/$MAIN...origin/$br" 2>/dev/null \
      | grep -qxF -e "$files" && culprits+=($p)
  done
  [ ${#culprits[@]} -gt 0 ] || return 1
  # Whole stacks: a culprit's group leaves with it.
  while read -r g; do
    hit=""
    for p in "${culprits[@]}"; do case " $g " in *" $p "*) hit=1;; esac; done
    if [ -n "$hit" ]; then leaving+=($g); else rest+=($g); fi
  done < <(groups "$@")
  [ ${#rest[@]} -gt 0 ] || return 1
  errs=$(grep "^    compile error: " "$out" | sed 's/^    compile error: //' | head -5 | tr '\n' ';' | sed 's/;$//; s/;/; /g')
  say "batch of $* does not compile: $errs"
  say "trying it without ${leaving[*]} (${culprits[*]} touch those files)"
  batch "${rest[@]}"
  for p in "${rest[@]}"; do
    [ "$(gh pr view $p --repo "$R" --json state --jq .state)" = MERGED ] && { merged=1; break; }
  done
  if [ -z "$merged" ]; then
    say "the rest did not land either; ${leaving[*]} keep their labels"
  else
    say "the rest landed; ${leaving[*]} go on as a batch of their own"
    batch "${leaving[@]}"
  fi
  return 0
}

# The labelled PRs that can land on their own: each one's commits, beyond main,
# may only include other PRs that are in the same list. One line each: the PR,
# its head, and whether it is batch-first.
eligible() {
  git -C "$G" fetch -q origin
  open=$(gh pr list --repo "$R" --state open --limit 1000 --json number,headRefName,headRefOid,labels \
         --jq '.[] | "\(.number) \(.headRefName) \(.headRefOid) \([.labels[].name] | index("batch-ready") != null) \([.labels[].name] | index("batch-first") != null)"')
  # `batch-first` PRs (e.g. a fix for a flaky test every batch would hit) go
  # ahead of the rest; then oldest first.
  ready=$( (echo "$open" | awk '$4=="true" && $5=="true" {print $1}' | sort -n
            echo "$open" | awk '$4=="true" && $5!="true" {print $1}' | sort -n) )
  for p in $ready; do
    head=$(echo "$open" | awk -v p=$p '$1==p {print $3}')
    # A head already on main has nothing to land, and GitHub may never mark
    # the PR merged (11-24: #1188, carried in by another PR's merge, its base
    # a merged PR's branch): batched, it left an empty commit on main every loop.
    if [ -n "$head" ] && git -C "$G" merge-base --is-ancestor "$head" "origin/$MAIN" 2>/dev/null; then
      unlabel $p "Batcher: this PR's head ${head:0:7} is already on main, so there is nothing to land. If GitHub has not marked it merged within a few minutes, close it." && \
      say "#$p left out: its head ${head:0:7} is already on main; label removed"
      continue
    fi
    # Its label could not be removed for this head: not batched again.
    grep -qxF "$p ${head:-unknown}" "$S/unlabel-failed" 2>/dev/null && continue
    ok=1
    while read -r o obr ohead olab; do
      [ "$o" = "$p" ] && continue
      echo " $ready " | tr '\n' ' ' | grep -q " $o " && continue
      if git -C "$G" merge-base --is-ancestor "$ohead" "$head" 2>/dev/null \
         && ! git -C "$G" merge-base --is-ancestor "$ohead" "origin/$MAIN" 2>/dev/null; then
        ok=0; break
      fi
    done <<< "$open"
    [ $ok = 1 ] && echo "$p $head $(echo "$open" | awk -v p=$p '$1==p {print $5}')"
  done | head -"$(cat "$S/batch-max" 2>/dev/null || echo $MAX)"
}

# One line per group of PRs that share commits beyond main (stacks), in input
# order. A split keeps each group whole: a stack's parent may not pass without
# its child (#443 needed #456 on 10-30, and the plain halving split them twice).
groups() {  # PR numbers
  local -a prs heads grp
  local i j k n=$# mb old new heads_all
  prs=("$@")
  heads_all=$(gh pr list --repo "$R" --state open --limit 1000 --json number,headRefOid --jq '.[] | "\(.number) \(.headRefOid)"')
  for ((i=0; i<n; i++)); do
    heads[$i]=$(echo "$heads_all" | awk -v p="${prs[$i]}" '$1==p {print $2}')
    grp[$i]=$i
  done
  for ((i=0; i<n; i++)); do
    for ((j=i+1; j<n; j++)); do
      [ "${grp[$i]}" = "${grp[$j]}" ] && continue
      mb=$(git -C "$G" merge-base "${heads[$i]}" "${heads[$j]}" 2>/dev/null) || continue
      git -C "$G" merge-base --is-ancestor "$mb" "origin/$MAIN" 2>/dev/null && continue
      old=${grp[$j]}; new=${grp[$i]}
      for ((k=0; k<n; k++)); do [ "${grp[$k]}" = "$old" ] && grp[$k]=$new; done
    done
  done
  for ((i=0; i<n; i++)); do
    [ "${grp[$i]}" = "done" ] && continue
    local g=${grp[$i]} line=""
    for ((k=0; k<n; k++)); do
      [ "${grp[$k]}" = "$g" ] && { line="$line ${prs[$k]}"; grp[$k]=done; }
    done
    echo $line
  done
}

batch() {  # PR numbers, or --resume, or --noboot and PR numbers; a failed batch is bisected
  [ $# -eq 0 ] && return
  # local: the halves below recurse, and shared `out`/`h` mis-split the batch
  local out h p files fails br noboot_run=""
  [ "$1" = --noboot ] && noboot_run=1
  # A resumed batch is a no-boot one when its branch says so (read before
  # batch-ff.sh clears the state), so its failure gets the full retry too.
  if [ "$1" = --resume ] && (. "$S/current-batch" 2>/dev/null; case "$name" in batch/noboot-*) exit 0;; *) exit 1;; esac); then
    noboot_run=1
  fi
  out="$S/batch-auto-$(date +%m%d-%H%M%S).out"
  bash "$KAI_SCRIPTS/batch-ff.sh" "$@" > "$out" 2>&1
  for p in $(grep -oE "^  - #[0-9]+ conflicts" "$out" | grep -oE "[0-9]+"); do
    files=$(awk -v p="#$p" '$0 ~ "^  - "p" conflicts" {f=1; next} /^  [-+] #/ {f=0} f' "$out" \
            | conflict_files | tr '\n' ' ')
    # A lane may have rebased while the batch ran: keep the label if the PR's
    # current head merges cleanly with current main.
    git -C "$G" fetch -q origin
    br=$(gh pr view $p --repo "$R" --json headRefName --jq .headRefName)
    if [ -n "$br" ] && git -C "$G" -c attr.tree="origin/$MAIN" merge-tree --write-tree "origin/$MAIN" "origin/$br" >/dev/null 2>&1; then
      say "#$p left out: conflicted at batch start, but its new head merges cleanly; label kept"
      continue
    fi
    unlabel $p "Batcher: left out of a merge batch because it conflicts with main in: $files. Rebase onto main, push once, and add \`batch-ready\` again."
    say "#$p left out: conflicts in $files"
  done
  for p in $(grep -oE "^  #[0-9]+: MERGED" "$out" | grep -oE "[0-9]+"); do say "#$p merged"; done
  # Its head is on main, but with its label kept the next loop lands it again.
  for p in $(grep -oE "^  #[0-9]+ landed but GitHub still shows it open" "$out" | grep -oE "[0-9]+"); do
    unlabel $p "Batcher: this PR's head is on main now, but GitHub still shows the PR open, so there is nothing left to land. If GitHub has not marked it merged within a few minutes, close it." && \
    say "#$p landed but GitHub still shows it open; label removed"
  done
  grep -q "CI did not finish" "$out" || rm -f "$S/batch-timeouts"
  grep -q "fast-forwarded" "$out" && return
  # Kept labels would have the batcher compose the same no-op batch every loop.
  if grep -q "adds nothing to main" "$out"; then
    for p in $(grep -oE "^  \+ #[0-9]+" "$out" | grep -oE "[0-9]+"); do
      unlabel $p "Batcher: merged with main, this PR's batch changes nothing (its change is already on main), so there is nothing to land. Close it, or mark it merged by hand." && \
      say "#$p left out: its batch adds nothing to main; label removed"
    done
    return
  fi
  # A wait that ran out is the runner queue, not the PRs (11-10: a batch with 12
  # of 14 jobs green and none red was split at the old cap). Keep the labels and
  # try again; two in a row stop the batcher so it does not feed a full queue.
  if grep -q "CI did not finish" "$out"; then
    local n=$(( $(cat "$S/batch-timeouts" 2>/dev/null || echo 0) + 1 ))
    echo $n > "$S/batch-timeouts"
    say "batch of $(grep -oE "^  \+ #[0-9]+" "$out" | grep -oE "[0-9]+" | tr '\n' ' ')ran out of time waiting for CI ($n in a row); labels kept"
    [ $n -ge 2 ] && { say "two in a row: stopping the batcher"; touch "$S/batch-auto.stop"; }
    return
  fi
  if grep -qE "NOT green|fluency below" "$out"; then
    set -- $(grep -oE "^  \+ #[0-9]+" "$out" | grep -oE "[0-9]+")
    [ $# -gt 1 ] && culprits_out "$out" "$@" && return
    # The boot cases that failed, as batch-ff.sh read them from the jobs' logs.
    local cases
    cases=$(grep "^    failed case: " "$out" | sed 's/^    failed case: xtask: //' | tr '\n' ';' | sed 's/;$//; s/;/; /g')
    [ -n "$cases" ] && say "batch of $* failed: $cases"
    # A no-boot batch may fail only because it needed the boot jobs after all
    # (CI's rule on the combined commit disagreed): its PRs go once more as a
    # full batch, a lone PR too, before anything is unlabelled or split.
    if [ -n "$noboot_run" ]; then
      say "no-boot batch of $* failed; trying it once as a full batch"
      batch "$@"
      return
    fi
    # The batch's one model PR failed the fluency gate (and nothing else
    # failed): it, and any PR stacked with it, leave the batch; the rest go
    # again without a split. The stack keeps its labels and waits on its parent.
    local -a model_prs
    model_prs=($(grep -oE "changes what the model sees \(( #[0-9]+)+\)" "$out" | grep -oE "[0-9]+"))
    if ! grep -q "NOT green" "$out" && [ ${#model_prs[@]} -eq 1 ] && [ $# -gt 1 ]; then
      local m=${model_prs[0]} g rest=()
      fails=$(grep -E "fluency run" "$out" | sed 's/^ *//' | tr '\n' ' ')
      if moved $m "$out"; then
        say "#$m failed the fluency gate, but it was pushed to since the batch took it; label kept"
      else
        unlabel $m "Batcher: this PR was its merge batch's only change to what the model sees, and the fluency gate failed: $fails. Fix it, push once, and add \`batch-ready\` again."
        say "#$m failed the fluency gate: $fails"
      fi
      while read -r g; do
        case " $g " in *" $m "*) ;; *) rest+=($g);; esac
      done < <(groups "$@")
      [ ${#rest[@]} -gt 0 ] && batch "${rest[@]}"
      return
    fi
    if [ $# -eq 1 ]; then
      if moved $1 "$out"; then
        say "#$1 failed alone, but it was pushed to since the batch took it; label kept"
        return
      fi
      fails=$(grep -E "=failure|fluency run|failed case: |compile error: " "$out" | sed 's/^ *//; s/^failed case: xtask: //' | tr '\n' ' ')
      unlabel $1 "Batcher: this PR failed in a merge batch on its own: $fails. Fix it, push once, and add \`batch-ready\` again."
      say "#$1 failed alone: $fails"
    elif [ $# -gt 1 ]; then
      say "batch of $* failed; splitting it"
      local -a gs first second
      local cnt=0 idx=0 half=$(( $# / 2 ))
      mapfile -t gs < <(groups "$@")
      if [ ${#gs[@]} -gt 1 ]; then
        # Whole groups into the first half until it holds about half the PRs;
        # the last group always goes to the second half.
        for g in "${gs[@]}"; do
          idx=$((idx + 1))
          if [ $cnt -lt $half ] && [ $idx -lt ${#gs[@]} ]; then
            first+=($g); cnt=$((cnt + $(echo $g | wc -w)))
          else
            second+=($g)
          fi
        done
        say "split by stack: [${first[*]}] [${second[*]}]"
        batch "${first[@]}"
        batch "${second[@]}"
      else
        h=$(( $# / 2 ))
        batch "${@:1:$h}"
        batch "${@:$((h+1))}"
      fi
    fi
    return
  fi
  say "batch stopped: $(grep -E 'STOPPING|nothing to merge' "$out" | head -1 | sed 's/^ *//')"
}

# A batch-ff.sh run is live while its pid file names a live process; a PC
# stop leaves the file, and its pid is gone.
ff_running() {
  local pid
  pid=$(cat "$S/batch-ff.pid" 2>/dev/null) && [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null
}

# Everything runs from here, once bash has read the whole file: a pull that
# rewrites this script cannot change the running batcher.
# Whether a head's whole change from main is one the boot jobs cannot see.
noboot_ok() {  # $1 head
  (cd "$G" && bash "$KAI_SCRIPTS/noboot.sh" "origin/$MAIN" "$1" >/dev/null)
}

main() {
  echo "batch-auto: state in $S (log, stop file, batch-max), repo $R, checkout $G"
  say "batcher started (state $S)"
  while [ ! -f "$S/batch-auto.stop" ]; do
    if ff_running; then sleep 120; continue; fi
    if [ -f "$S/current-batch" ]; then
      say "resuming the unfinished batch: $(. "$S/current-batch"; echo "$prs")"
      batch --resume
      # batch-ff.sh clears the state on every way out; a state left behind means
      # it died, so park it rather than resume it forever.
      if [ -f "$S/current-batch" ]; then
        mv -f "$S/current-batch" "$S/current-batch.failed-$(date +%m%d-%H%M%S)"
        say "the resume did not finish; state parked beside the log, labels kept"
      fi
      sleep 30
      continue
    fi
    rows=$(eligible)
    if [ -z "$rows" ]; then sleep 300; continue; fi
    list=$(echo "$rows" | awk '{print $1}')
    # A no-boot batch, which needs lint alone, only when every eligible PR is
    # one the boot jobs cannot see: with any boot PR waiting, one full batch
    # takes them all, as a no-boot batch first would cost its landing's full
    # run on main as well (#1108's review). A failed no-boot batch is tried
    # once as a full batch before anything is unlabelled or split.
    if [ "$NOBOOT" = 1 ]; then
      nb=$(echo "$rows" | while read -r p h f; do noboot_ok "$h" && echo $p; done)
      if [ -n "$nb" ] && [ "$(echo $nb)" = "$(echo $list)" ]; then
        say "no-boot batch: $(echo $nb)"
        batch --noboot $nb
        sleep 30
        continue
      fi
    fi
    say "batch: $(echo $list)"
    batch $list
    sleep 30
  done
  say "batcher stopped"
}

main "$@"; exit
