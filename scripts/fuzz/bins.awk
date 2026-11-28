# fuzz/Cargo.toml's [[bin]] tables as "name path" lines, for the Fuzz
# workflow's plan job (#1585's review): `name="x"`, `name = 'x'` and a
# trailing comment all read, in either order, and a key of another table
# ([package]'s name, a dependency's path) never counts.
# Usage: awk -f scripts/fuzz/bins.awk fuzz/Cargo.toml
function value(line) {
  sub(/^[a-z]+[ \t]*=[ \t]*/, "", line)
  sub(/[ \t]*(#.*)?$/, "", line)
  gsub(/["']/, "", line)
  return line
}
function flush() { if (name != "" && path != "") print name, path; name = ""; path = "" }
/^[ \t]*\[/ { flush(); inbin = ($0 ~ /^[ \t]*\[\[bin\]\]/); next }
inbin && /^name[ \t]*=/ { name = value($0) }
inbin && /^path[ \t]*=/ { path = value($0) }
END { flush() }
