#!/usr/bin/env bash
# For host-only PRs (no full-ci, so no boot jobs): label batch-ready, in order,
# once ci.yml's "Format, lint, host unit tests" passes (DCO is a step of it) on the expected head.
# Exits early if either fails or the head moves.
# Usage: label-on-lint.sh <pr>:<head-prefix> [...]   (one gh round per 5 min)
# Settings: config.sh.
set -u
. "$(dirname "$0")/config.sh"
. "$KAI_SCRIPTS/checks-lib.sh"
for arg in "$@"; do
  pr=${arg%%:*}; want=${arg#*:}
  while :; do
    head=$(gh pr view "$pr" --repo "$R" --json headRefOid --jq .headRefOid 2>/dev/null)
    [ -z "$head" ] && { sleep 60; continue; }
    case "$head" in "$want"*) ;; *) echo "#$pr head moved to ${head:0:8} (wanted $want); stopping"; exit 1;; esac
    run=$(gh run list --repo "$R" --workflow ci.yml --commit "$head" --limit 5 --json databaseId,event,conclusion \
          --jq '[.[]|select(.event!="push" and .conclusion!="cancelled" and .conclusion!="skipped")][0].databaseId // .[0].databaseId' 2>/dev/null)
    if [ -z "$run" ] && [ "$(gh pr view "$pr" --repo "$R" --json mergeable --jq .mergeable 2>/dev/null)" = CONFLICTING ]; then
      echo "#$pr conflicts with main, so CI will not run; stopping"; exit 1
    fi
    if [ -n "$run" ]; then
      jobs=$(gh run view "$run" --repo "$R" --json jobs \
             --jq '.jobs[]|select(.name=="Format, lint, host unit tests")|.name+"="+(.conclusion//"")' 2>/dev/null)
      if echo "$jobs" | grep -qE "=(failure|cancelled|timed_out)$"; then
        echo "#$pr run $run: lint failed:"; echo "$jobs"; exit 1
      fi
      if [ "$(echo "$jobs" | grep -c '=success$')" = 1 ]; then
        # Every other check on the head too (11-05: #696's pkgrepo was red).
        all=$(all_checks "$pr")
        case "$all" in fail*) echo "#$pr: lint passes but another check failed: $all"; exit 1;; esac
        if [ "$all" = ok ]; then
          gh pr ready "$pr" --repo "$R" >/dev/null 2>&1
          gh pr edit "$pr" --repo "$R" --add-label batch-ready >/dev/null && echo "$(date -u +%H:%MZ) #$pr labelled batch-ready (run $run, lint + all checks)"
          break
        fi
      fi
    fi
    sleep 300
  done
done
