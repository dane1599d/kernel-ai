# Sourced by label-on-kvm.sh and label-on-lint.sh (after config.sh, for $R).
# all_checks <pr>: prints "ok" when no check on the PR's head is failing or
# pending, "pending" while any is still running, or "fail: <names>" when one
# failed. Added 11-05: #696 was labelled on lint alone while its own pkgrepo
# workflow (a TUF tamper check) was red, and it broke batch #713.
# A check still queued, running or not listed yet is never fine: no rows at
# all reads as pending. A cancelled check (a label event cancels its twin run)
# is fine only when a run of the same name passed or was skipped; one that
# never finished anywhere (a cancelled Oracle run, say) fails, as #1196 was
# labelled with its Oracle run unfinished (11-19, the manager's Temp watcher).
all_checks() {
  local rows failed unfinished
  rows=$(gh pr checks "$1" --repo "$R" --json name,bucket --jq '.[] | .bucket + "|" + .name' 2>/dev/null)
  if [ -z "$rows" ]; then echo pending; return; fi
  failed=$(echo "$rows" | awk -F'|' '$1 == "fail" { printf "%s; ", $2 }')
  if [ -n "$failed" ]; then echo "fail: $failed"; return; fi
  if echo "$rows" | grep -qv '^\(pass\|skipping\|cancel\)|'; then echo pending; return; fi
  unfinished=$(echo "$rows" | awk -F'|' '
    $1 == "pass" || $1 == "skipping" { done[$2] = 1 }
    $1 == "cancel" { cancelled[$2] = 1 }
    END { for (n in cancelled) if (!(n in done)) printf "%s; ", n }')
  if [ -n "$unfinished" ]; then echo "fail: cancelled, never passed: $unfinished"; return; fi
  echo ok
}
