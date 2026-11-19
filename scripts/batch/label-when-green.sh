#!/usr/bin/env bash
# The manager's label watch: labels each PR batch-ready (and marks it ready)
# once every check on its expected head has passed, lint included. A PR whose
# head moved or whose check failed is reported and dropped; the rest are kept.
# With --together, the PRs are a stack, listed parent first: a failure or moved
# head anywhere stops the watch, and all are labelled when all are green, the
# last child first, so the batcher never sees a labelled parent without them.
# Usage: label-when-green.sh [--together] [--note TEXT] <pr>:<head-prefix> ...
# A head prefix has 7 to 40 hex digits. One round of gh calls per 5 minutes (the
# GraphQL budget is shared); gives up after KAI_WATCH_ROUNDS rounds, default 72
# (6 hours: a full-ci PR's boots can queue for hours). Settings: config.sh.
. "$(dirname "$0")/config.sh"
together=0
note="Labelled batch-ready (manager)."
while :; do
  case "$1" in
    --together) together=1; note="Stack labelled batch-ready together (manager)."; shift;;
    --note) note=$2; shift 2;;
    *) break;;
  esac
done
[ $# -gt 0 ] || { echo "usage: label-when-green.sh [--together] [--note TEXT] <pr>:<sha> ..."; exit 2; }
todo="$*"
for item in $todo; do
  [[ "$item" =~ ^[0-9]+:[0-9a-f]{7,40}$ ]] || { echo "$item: want <pr>:<7 to 40 hex digits of its head>"; exit 2; }
done

label() {  # $1 PR, $2 sha
  gh pr ready "$1" --repo "$R" >/dev/null 2>&1
  gh pr edit "$1" --repo "$R" --add-label batch-ready >/dev/null \
    && gh pr comment "$1" --repo "$R" --body "Green at $2. $note" >/dev/null \
    && echo "#$1 labelled batch-ready at $2"
}

for i in $(seq 1 "${KAI_WATCH_ROUNDS:-72}"); do
  left=""; green=""; bad=""
  for item in $todo; do
    n=${item%%:*}; sha=${item##*:}
    head=$(gh pr view "$n" --repo "$R" --json headRefOid --jq .headRefOid 2>/dev/null)
    # An empty read is a GitHub hiccup, not a moved head: try again next round.
    [ -z "$head" ] && { left="$left $item"; continue; }
    case "$head" in
      "$sha"*) ;;
      *) echo "#$n head moved to ${head:0:8}; not labelling"
         [ $together = 1 ] && { echo "stack not labelled"; exit 0; }
         continue;;
    esac
    out=$(gh pr checks "$n" --repo "$R" 2>&1 | grep -viE skipping | grep -vF '${{')
    if echo "$out" | grep -qiE $'\t(fail|cancel)'; then
      echo "#$n FAILED:"; echo "$out" | grep -iE $'\t(fail|cancel)'; bad="$bad #$n"; continue
    fi
    if echo "$out" | grep -qiE $'\tpending'; then left="$left $item"; continue; fi
    # lint must be listed and passed: checks that have not started yet are not green
    if ! echo "$out" | grep -qE $'^Format, lint, host unit tests\tpass'; then left="$left $item"; continue; fi
    if [ $together = 1 ]; then green="$green $item"; else label "$n" "$sha"; fi
  done
  if [ $together = 1 ]; then
    [ -n "$bad" ] && { echo "stack not labelled: failures in$bad"; exit 0; }
    if [ -z "$left" ]; then
      # Children first: the batcher takes a stack only once all of it is labelled.
      for item in $(printf '%s\n' $green | tac); do label "${item%%:*}" "${item##*:}"; done
      exit 0
    fi
  else
    todo=$left
    [ -z "$todo" ] && exit 0
  fi
  sleep 300
done
echo "timed out; still pending:$left"
