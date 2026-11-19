# Sourced by label-on-kvm.sh and label-on-lint.sh (after config.sh, for $R).
# all_checks <pr>: prints "ok" when no check on the PR's head is failing or
# pending, "pending" while any is still running, or "fail: <names>" when one
# failed. Added 11-05: #696 was labelled on lint alone while its own pkgrepo
# workflow (a TUF tamper check) was red, and it broke batch #713.
# A cancelled check (a label event cancels its twin run) neither passes nor
# blocks here; the callers still require their own jobs to succeed.
all_checks() {
  local rows failed
  rows=$(gh pr checks "$1" --repo "$R" --json name,bucket --jq '.[] | .bucket + "|" + .name' 2>/dev/null)
  if [ -z "$rows" ]; then echo pending; return; fi
  failed=$(echo "$rows" | awk -F'|' '$1 == "fail" { printf "%s; ", $2 }')
  if [ -n "$failed" ]; then echo "fail: $failed"; return; fi
  if echo "$rows" | grep -q '^pending|'; then echo pending; return; fi
  echo ok
}
