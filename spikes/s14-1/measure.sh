#!/usr/bin/env bash
# S14-1: every fixture, natively and on each runtime, each in a process of its
# own, RUNS renders a process. Writes raw lines to $OUT/raw.txt and the report
# to $OUT/report.md. Run build.sh first.
set -euo pipefail
cd "$(dirname "$0")"
RUNS=${RUNS:-5}
OUT=${OUT:-out}
FIXTURES=${FIXTURES:-"hello article table flex grid form docs rules inline deep"}
MODES=${MODES:-"native aot aot-scalar pulley wasmi"}
T=target
mkdir -p "$OUT"
: > "$OUT/raw.txt"
P2=$T/simd/wasm32-wasip2/release/s14-engine.wasm
P2S=$T/wasm32-wasip2/release/s14-engine.wasm
P1=$T/wasm32-wasip1/release/s14-engine.wasm
RUN=$T/release/s14-runner
for m in aot aot-scalar pulley; do
  case " $MODES " in *" $m "*)
    src=$P2; kind=compile-aot
    [ "$m" = aot-scalar ] && src=$P2S
    [ "$m" = pulley ] && kind=compile-pulley
    "$RUN" "$kind" "$src" "$OUT/$m.cwasm" | sed "s/^/compile-$m /" >> "$OUT/raw.txt";;
  esac
done
# Runs "$@" with its stdout to $OUT/one.txt; prints its peak RSS in MiB.
timed() {
  if [ ! -x /usr/bin/time ]; then
    "$@" > "$OUT/one.txt"; echo 0
  elif /usr/bin/time -v true >/dev/null 2>&1; then
    /usr/bin/time -v "$@" > "$OUT/one.txt" 2> "$OUT/time.txt"
    awk '/Maximum resident/ {print int($6/1024)}' "$OUT/time.txt"
  else
    /usr/bin/time -l "$@" > "$OUT/one.txt" 2> "$OUT/time.txt"
    awk '/maximum resident/ {print int($1/1048576)}' "$OUT/time.txt"
  fi
}
for f in $FIXTURES; do
  for m in $MODES; do
    case $m in
      native)
        mib=$(timed $T/release/s14-engine "$f" "$RUNS")
        sed "s/^/$m /" "$OUT/one.txt" >> "$OUT/raw.txt"
        echo "$m s14-host mode=native fixture=$f peak_rss_mib=$mib" >> "$OUT/raw.txt";;
      aot|aot-scalar|pulley) { "$RUN" "${m%-scalar}" "$OUT/$m.cwasm" "$f" "$RUNS" || true; } | sed "s/^/$m /; s/s14-host mode=[a-z]*/s14-host mode=$m fixture=$f/" >> "$OUT/raw.txt";;
      wasmi) { "$RUN" wasmi "$P1" "$f" "$RUNS" || true; } | sed "s/^/$m /; s/s14-host mode=$m/s14-host mode=$m fixture=$f/" >> "$OUT/raw.txt";;
    esac
    echo "done $f $m" >&2
  done
done
python3 report.py "$OUT/raw.txt" > "$OUT/report.md"
cat "$OUT/report.md"
