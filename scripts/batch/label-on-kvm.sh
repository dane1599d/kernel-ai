#!/usr/bin/env bash
# Label PRs batch-ready, in order, once the 3 kvm boot shards and lint pass on each PR's
# expected head. Exits early (to wake the manager) if a shard fails or a head moves.
# Usage: label-on-kvm.sh <pr>:<head-prefix> [...]   (one gh round per 5 min)
# Settings: config.sh.
set -u
. "$(dirname "$0")/config.sh"
. "$KAI_SCRIPTS/checks-lib.sh"
for arg in "$@"; do
  pr=${arg%%:*}; want=${arg#*:}
  while :; do
    head=$(gh pr view "$pr" --repo "$R" --json headRefOid --jq .headRefOid 2>/dev/null)
    # An empty read is a GitHub hiccup, not a moved head (11-03: stopped #568's watch).
    [ -z "$head" ] && { sleep 60; continue; }
    case "$head" in "$want"*) ;; *) echo "#$pr head moved to ${head:0:8} (wanted $want); stopping"; exit 1;; esac
    # Skip a run cancelled by its twin (a label event and a push share one
    # concurrency group): it has no boot jobs, so it would be waited on forever (#600, 11-03).
    run=$(gh run list --repo "$R" --workflow ci.yml --commit "$head" --limit 5 --json databaseId,event,conclusion \
          --jq '[.[]|select(.event!="push" and .conclusion!="cancelled" and .conclusion!="skipped")][0].databaseId // .[0].databaseId' 2>/dev/null)
    # A PR that conflicts with main gets no CI run at all: say so rather than
    # wait forever (#607 and #610 on 11-03).
    if [ -z "$run" ] && [ "$(gh pr view "$pr" --repo "$R" --json mergeable --jq .mergeable 2>/dev/null)" = CONFLICTING ]; then
      echo "#$pr conflicts with main, so CI will not run; stopping"; exit 1
    fi
    if [ -n "$run" ]; then
      jobs=$(gh run view "$run" --repo "$R" --json jobs \
             --jq '.jobs[]|select(.name|test("^Boot test \\(kvm, shard|^Format, lint, host unit tests"))|.name+"="+(.conclusion//"")' 2>/dev/null)
      if echo "$jobs" | grep -qE "=(failure|cancelled|timed_out)$"; then
        echo "#$pr run $run: a kvm shard or lint failed:"; echo "$jobs"; exit 1
      fi
      # Lint must pass too (11-04: #666 passed kvm 3/3 but failed its licences check).
      if [ "$(echo "$jobs" | grep -c '^Boot test.*=success$')" = 3 ] && echo "$jobs" | grep -q '^Format, lint, host unit tests=success$'; then
        # Every other check on the head too (11-05: #696's pkgrepo was red).
        all=$(all_checks "$pr")
        case "$all" in fail*) echo "#$pr: kvm + lint pass but another check failed: $all"; exit 1;; esac
        if [ "$all" = ok ]; then
          gh pr ready "$pr" --repo "$R" >/dev/null 2>&1
          gh pr edit "$pr" --repo "$R" --add-label batch-ready >/dev/null && echo "$(date -u +%H:%MZ) #$pr labelled batch-ready (run $run, 3/3 kvm + lint + all checks)"
          break
        fi
      fi
    fi
    sleep 300
  done
done
