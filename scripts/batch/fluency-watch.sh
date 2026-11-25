#!/usr/bin/env bash
# Dispatches fluency-model.yml (default inputs: the rhai fluency gate) on a PR
# branch, waits for the run on the given head, and prints the verdict.
# Usage: bash fluency-watch.sh <branch> <full sha>
# One gh call per 5 minutes while waiting (the GraphQL budget is shared).
# The run's log is saved in $S. Settings: config.sh.
. "$(dirname "$0")/config.sh"
BR="$1"
SHA="$2"
if [ -z "$BR" ] || [ -z "$SHA" ]; then echo "usage: fluency-watch.sh <branch> <sha>"; exit 2; fi

gh workflow run fluency-model.yml --repo "$R" --ref "$BR" || { echo "dispatch failed"; exit 1; }
echo "dispatched fluency-model.yml on $BR at $(date -u +%H:%MZ)"
sleep 60

id=""
state=""
for i in $(seq 1 26); do
  f=$(gh run list --repo "$R" --workflow fluency-model.yml --branch "$BR" --limit 3 \
        --json headSha,status,databaseId,conclusion \
        --jq ".[] | select(.headSha==\"$SHA\") | (.databaseId|tostring)+\" \"+.status+\" \"+(.conclusion // \"\")" | head -1)
  id=${f%% *}
  state=$f
  case "$f" in *completed*) break ;; esac
  sleep 300
done
if [ -z "$id" ]; then echo "no run found for $SHA on $BR"; exit 1; fi
echo "run $state at $(date -u +%H:%MZ)"

LOG="$S/fluency-$id.log"
# The log can be incomplete right after the run ends: read until the verdict is in.
for i in 1 2 3 4 5 6; do
  gh run view "$id" --repo "$R" --log > "$LOG" 2>/dev/null
  grep -qE "the fluency gate (passes|fails)" "$LOG" && break
  sleep 45
done
line=$(grep -oE "fluency: [0-9]+/50 first try \([0-9]+%\), [0-9]+/50 after one repair \([0-9]+%\)" "$LOG" | head -1)
verdict=$(grep -oE "the fluency gate (passes|fails)" "$LOG" | head -1)
echo "fluency run $id on $BR ($SHA): $line; $verdict"
pct=$(echo "$line" | sed -nE 's/.*after one repair \(([0-9]+)%\).*/\1/p')
if [ -n "$pct" ] && [ "$pct" -ge "$FLUENCY_MIN" ] && [ "$verdict" = "the fluency gate passes" ]; then
  echo "RESULT: PASS"
else
  echo "RESULT: NOT PASSING (read $LOG)"
  exit 1
fi
