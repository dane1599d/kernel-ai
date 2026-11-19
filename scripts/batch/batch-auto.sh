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

unlabel() {  # $1 PR, $2 comment
  gh pr edit "$1" --repo "$R" --remove-label batch-ready >/dev/null 2>&1
  gh pr comment "$1" --repo "$R" --body "$2" >/dev/null 2>&1
}

# The labelled PRs that can land on their own: each one's commits, beyond main,
# may only include other PRs that are in the same list.
eligible() {
  git -C "$G" fetch -q origin
  open=$(gh pr list --repo "$R" --state open --limit 80 --json number,headRefName,headRefOid,labels \
         --jq '.[] | "\(.number) \(.headRefName) \(.headRefOid) \([.labels[].name] | index("batch-ready") != null) \([.labels[].name] | index("batch-first") != null)"')
  # `batch-first` PRs (e.g. a fix for a flaky test every batch would hit) go
  # ahead of the rest; then oldest first.
  ready=$( (echo "$open" | awk '$4=="true" && $5=="true" {print $1}' | sort -n
            echo "$open" | awk '$4=="true" && $5!="true" {print $1}' | sort -n) )
  for p in $ready; do
    head=$(echo "$open" | awk -v p=$p '$1==p {print $3}')
    ok=1
    while read -r o obr ohead olab; do
      [ "$o" = "$p" ] && continue
      echo " $ready " | tr '\n' ' ' | grep -q " $o " && continue
      if git -C "$G" merge-base --is-ancestor "$ohead" "$head" 2>/dev/null \
         && ! git -C "$G" merge-base --is-ancestor "$ohead" "origin/$MAIN" 2>/dev/null; then
        ok=0; break
      fi
    done <<< "$open"
    [ $ok = 1 ] && echo $p
  done | head -"$(cat "$S/batch-max" 2>/dev/null || echo $MAX)"
}

# One line per group of PRs that share commits beyond main (stacks), in input
# order. A split keeps each group whole: a stack's parent may not pass without
# its child (#443 needed #456 on 10-30, and the plain halving split them twice).
groups() {  # PR numbers
  local -a prs heads grp
  local i j k n=$# mb old new heads_all
  prs=("$@")
  heads_all=$(gh pr list --repo "$R" --state open --limit 80 --json number,headRefOid --jq '.[] | "\(.number) \(.headRefOid)"')
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

batch() {  # PR numbers, or --resume; a failed batch is bisected
  [ $# -eq 0 ] && return
  # local: the halves below recurse, and shared `out`/`h` mis-split the batch
  local out h p files fails br
  out="$S/batch-auto-$(date +%m%d-%H%M%S).out"
  bash "$KAI_SCRIPTS/batch-ff.sh" "$@" > "$out" 2>&1
  for p in $(grep -oE "^  - #[0-9]+ conflicts" "$out" | grep -oE "[0-9]+"); do
    files=$(awk -v p="#$p" '$0 ~ "^  - "p" conflicts" {f=1; next} /^  [-+] #/ {f=0} f' "$out" \
            | grep -oE "Merge conflict in .*" | sed 's/Merge conflict in //' | tr '\n' ' ')
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
  grep -q "CI did not finish" "$out" || rm -f "$S/batch-timeouts"
  grep -q "fast-forwarded" "$out" && return
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
    # The boot cases that failed, as batch-ff.sh read them from the jobs' logs.
    local cases
    cases=$(grep "^    failed case: " "$out" | sed 's/^    failed case: xtask: //' | tr '\n' ';' | sed 's/;$//; s/;/; /g')
    [ -n "$cases" ] && say "batch of $* failed: $cases"
    # The batch's one model PR failed the fluency gate (and nothing else
    # failed): it, and any PR stacked with it, leave the batch; the rest go
    # again without a split. The stack keeps its labels and waits on its parent.
    local -a model_prs
    model_prs=($(grep -oE "changes what the model sees \(( #[0-9]+)+\)" "$out" | grep -oE "[0-9]+"))
    if ! grep -q "NOT green" "$out" && [ ${#model_prs[@]} -eq 1 ] && [ $# -gt 1 ]; then
      local m=${model_prs[0]} g rest=()
      fails=$(grep -E "fluency run" "$out" | sed 's/^ *//' | tr '\n' ' ')
      unlabel $m "Batcher: this PR was its merge batch's only change to what the model sees, and the fluency gate failed: $fails. Fix it, push once, and add \`batch-ready\` again."
      say "#$m failed the fluency gate: $fails"
      while read -r g; do
        case " $g " in *" $m "*) ;; *) rest+=($g);; esac
      done < <(groups "$@")
      [ ${#rest[@]} -gt 0 ] && batch "${rest[@]}"
      return
    fi
    if [ $# -eq 1 ]; then
      fails=$(grep -E "=failure|fluency run|failed case: " "$out" | sed 's/^ *//; s/^failed case: xtask: //' | tr '\n' ' ')
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
    list=$(eligible)
    if [ -z "$list" ]; then sleep 300; continue; fi
    say "batch: $(echo $list)"
    batch $list
    sleep 30
  done
  say "batcher stopped"
}

main "$@"; exit
