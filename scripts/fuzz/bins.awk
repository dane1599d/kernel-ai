# fuzz/Cargo.toml's [[bin]] tables as "name path" lines, for the Fuzz
# workflow's plan job (#1585's review): `name="x"`, `name = 'x'` and a
# trailing comment all read, in either order, indented or not, and a key of
# another table ([package]'s name, a dependency's path) never counts. A
# quoted value ends at its closing quote, so a `#` inside it is kept. In a
# "..." value a backslash escapes the next character, so `\"` and `\\` read
# as TOML has them (no bin name or path holds another escape); a '...' value
# has no escapes, as TOML's literal strings. A value whose quote never closes
# is an error: nothing is printed and the exit status is 1 (#1657's review).
# Usage: awk -f scripts/fuzz/bins.awk fuzz/Cargo.toml
function unclosed() {
  printf "bins.awk: %s:%d: a quoted value with no closing quote\n", FILENAME, FNR > "/dev/stderr"
  bad = 1
  exit 1
}
function value(line,   q, i, c, out) {
  sub(/^[ \t]*[a-z]+[ \t]*=[ \t]*/, "", line)
  q = substr(line, 1, 1)
  if (q == "'") {
    i = index(substr(line, 2), q)
    if (!i) unclosed()
    return substr(line, 2, i - 1)
  }
  if (q == "\"") {
    out = ""
    for (i = 2; i <= length(line); i++) {
      c = substr(line, i, 1)
      if (c == "\"") return out
      if (c == "\\") { i++; c = substr(line, i, 1) }
      out = out c
    }
    unclosed()
  }
  sub(/[ \t]*(#.*)?$/, "", line)
  return line
}
function flush() { if (name != "" && path != "") print name, path; name = ""; path = "" }
/^[ \t]*\[/ { flush(); inbin = ($0 ~ /^[ \t]*\[\[bin\]\]/); next }
inbin && /^[ \t]*name[ \t]*=/ { name = value($0) }
inbin && /^[ \t]*path[ \t]*=/ { path = value($0) }
END { if (bad) exit 1; flush() }
