# fuzz/Cargo.toml's [[bin]] tables as "name path" lines, for the Fuzz
# workflow's plan job (#1585's review): `name="x"`, `name = 'x'` and a
# trailing comment all read, in either order, indented or not, and a key of
# another table ([package]'s name, a dependency's path) never counts. A
# quoted value ends at its closing quote, so a `#` inside it is kept.
# Usage: awk -f scripts/fuzz/bins.awk fuzz/Cargo.toml
function value(line,   q, end) {
  sub(/^[ \t]*[a-z]+[ \t]*=[ \t]*/, "", line)
  q = substr(line, 1, 1)
  if (q == "\"" || q == "'") {
    end = index(substr(line, 2), q)
    return end ? substr(line, 2, end - 1) : substr(line, 2)
  }
  sub(/[ \t]*(#.*)?$/, "", line)
  return line
}
function flush() { if (name != "" && path != "") print name, path; name = ""; path = "" }
/^[ \t]*\[/ { flush(); inbin = ($0 ~ /^[ \t]*\[\[bin\]\]/); next }
inbin && /^[ \t]*name[ \t]*=/ { name = value($0) }
inbin && /^[ \t]*path[ \t]*=/ { path = value($0) }
END { flush() }
