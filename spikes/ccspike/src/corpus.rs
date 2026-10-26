//! The spike's C: `kai.h`, preludes of 0, 200 and 1,000 lines, the 20-entry corpus
//! (1 to 150 lines, the `notes` component's logic among them), the sieve and the
//! sum, and the redefinition sequence of measure (g).
//!
//! Each entry is what promptcore would hand `cc` after its conveniences: plain C
//! with a `__kai_run_N` function where the entry runs something. Each entry that
//! runs states the value its run function must return.

use alloc::format;
use alloc::string::String;

/// The spike's `kai.h`: HolyC's type names and the three `kai` imports.
pub const KAI_H: &str = r#"#ifndef KAI_H
#define KAI_H
typedef void U0;
typedef signed char I8;
typedef short I16;
typedef int I32;
typedef long long I64;
typedef unsigned char U8;
typedef unsigned short U16;
typedef unsigned int U32;
typedef unsigned long long U64;
typedef double F64;
typedef int Bool;
#define TRUE 1
#define FALSE 0
#define NULL ((void*)0)
__attribute__((import_module("kai"), import_name("print_i64"))) void kai_print_i64(I64 x);
__attribute__((import_module("kai"), import_name("print_str"))) void kai_print_str(const char *s, I32 n);
__attribute__((import_module("kai"), import_name("need"))) I64 kai_need(I64 x);
static inline I32 kai_strlen(const char *s) { I32 n = 0; while (s[n]) ++n; return n; }
#define Print(s) kai_print_str((s), kai_strlen(s))
#endif
"#;

/// A prelude of about `lines` lines, as promptcore writes one for a long session:
/// types, macros, and a prototype and an `extern` for each live definition.
pub fn prelude(lines: usize) -> String {
    let mut out = String::from("#include <kai.h>\n");
    let groups = lines / 10;
    for i in 0..groups {
        out.push_str(&format!(
            "#define LIMIT_{i} ({i} * 16 + 3)\n\
             typedef struct Rec{i} {{\n  I32 id;\n  I64 total;\n  U8 tag[8];\n}} Rec{i};\n\
             enum Kind{i} {{ KIND{i}_A, KIND{i}_B = LIMIT_{i} }};\n\
             I64 rec{i}_sum(const Rec{i} *r, I32 n);\n\
             extern I64 rec{i}_count;\n"
        ));
    }
    out
}

/// One corpus entry.
pub struct Entry {
    pub name: &'static str,
    pub source: &'static str,
    /// The run function and the value it must return.
    pub run: Option<(&'static str, i64)>,
}

const fn entry(
    name: &'static str,
    source: &'static str,
    run: Option<(&'static str, i64)>,
) -> Entry {
    Entry { name, source, run }
}

/// 20 entries, run in order in one session: later ones use earlier definitions.
pub const CORPUS: &[Entry] = &[
    entry(
        "expr",
        "I64 __kai_run_1(void) { return 1 + 2 * 3; }\n",
        Some(("__kai_run_1", 7)),
    ),
    entry("locals", ENTRY_LOCALS, Some(("__kai_run_2", 55))),
    entry("define-sq", "I32 sq(I32 x) { return x * x; }\n", None),
    entry(
        "use-sq",
        "I32 sq(I32 x);\nI64 __kai_run_4(void) { return sq(7); }\n",
        Some(("__kai_run_4", 49)),
    ),
    entry("global", "I64 counter = 5;\n", None),
    entry(
        "extern",
        "extern I64 counter;\nI64 __kai_run_6(void) { counter += 3; return counter; }\n",
        Some(("__kai_run_6", 8)),
    ),
    entry(
        "print",
        "I64 __kai_run_7(void) { Print(\"hello\\n\"); kai_print_i64(42); return 5; }\n",
        Some(("__kai_run_7", 5)),
    ),
    entry("struct", ENTRY_STRUCT, Some(("__kai_run_8", 30))),
    entry("fib", ENTRY_FIB, Some(("__kai_run_9", 6765))),
    entry("switch", ENTRY_SWITCH, Some(("__kai_run_10", 3))),
    entry("array", ENTRY_ARRAY, Some(("__kai_run_11", 4950))),
    entry("fnptr", ENTRY_FNPTR, Some(("__kai_run_12", 21))),
    entry(
        "need",
        "I64 __kai_run_13(void) { return kai_need(41) + kai_need(0); }\n",
        Some(("__kai_run_13", 43)),
    ),
    entry("strings", ENTRY_STRINGS, Some(("__kai_run_14", 1))),
    entry("sort", ENTRY_SORT, Some(("__kai_run_15", 1))),
    entry("hash", ENTRY_HASH, Some(("__kai_run_16", 300))),
    entry("calc", ENTRY_CALC, Some(("__kai_run_17", 47))),
    entry("bits", ENTRY_BITS, Some(("__kai_run_18", 32))),
    entry("notes", ENTRY_NOTES, Some(("__kai_run_19", 1))),
    entry(
        "uses-many",
        ENTRY_USES,
        Some(("__kai_run_20", 49 + 8 + 6765)),
    ),
];

const ENTRY_LOCALS: &str = "I64 __kai_run_2(void) {
  I64 total = 0;
  for (I32 i = 1; i <= 10; ++i)
    total += i;
  return total;
}
";

const ENTRY_STRUCT: &str = "typedef struct { I32 x, y; } Point;
static I32 dot(const Point *a, const Point *b) { return a->x * b->x + a->y * b->y; }
I64 __kai_run_8(void) {
  Point a = {2, 3}, b = {3, 8};
  return dot(&a, &b);
}
";

const ENTRY_FIB: &str = "I64 fib(I64 n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
I64 __kai_run_9(void) { return fib(20); }
";

const ENTRY_SWITCH: &str = "static I32 vowels(const char *s) {
  I32 n = 0;
  for (; *s; ++s) {
    switch (*s) {
    case 'a': case 'e': case 'i': case 'o': case 'u':
      ++n;
      break;
    default:
      break;
    }
  }
  return n;
}
I64 __kai_run_10(void) { return vowels(\"kernel ai os\"); }
";

const ENTRY_ARRAY: &str = "I32 table[100];
I64 __kai_run_11(void) {
  I64 s = 0;
  for (I32 i = 0; i < 100; ++i) table[i] = i;
  for (I32 i = 0; i < 100; ++i) s += table[i];
  return s;
}
";

const ENTRY_FNPTR: &str = "static I64 add(I64 a, I64 b) { return a + b; }
static I64 mul(I64 a, I64 b) { return a * b; }
typedef I64 (*Op)(I64, I64);
Op ops[2] = {add, mul};
I64 __kai_run_12(void) { return ops[0](3, 4) * ops[1](1, 3); }
";

const ENTRY_STRINGS: &str = "static I32 str_eq(const char *a, const char *b) {
  while (*a && *a == *b) { ++a; ++b; }
  return *a == *b;
}
static void reverse(char *s) {
  I32 n = kai_strlen(s);
  for (I32 i = 0; i < n / 2; ++i) {
    char t = s[i];
    s[i] = s[n - 1 - i];
    s[n - 1 - i] = t;
  }
}
static void itoa10(I64 v, char *out) {
  char tmp[24];
  I32 n = 0;
  Bool neg = v < 0;
  if (neg) v = -v;
  do { tmp[n++] = '0' + v % 10; v /= 10; } while (v);
  if (neg) tmp[n++] = '-';
  for (I32 i = 0; i < n; ++i) out[i] = tmp[n - 1 - i];
  out[n] = 0;
}
I64 __kai_run_14(void) {
  char buf[32] = \"stressed\";
  reverse(buf);
  if (!str_eq(buf, \"desserts\")) return 0;
  itoa10(-1234, buf);
  return str_eq(buf, \"-1234\");
}
";

const ENTRY_SORT: &str = "static void sort(I32 *a, I32 n) {
  for (I32 i = 1; i < n; ++i) {
    I32 v = a[i], j = i - 1;
    while (j >= 0 && a[j] > v) { a[j + 1] = a[j]; --j; }
    a[j + 1] = v;
  }
}
static I32 find(const I32 *a, I32 n, I32 key) {
  I32 lo = 0, hi = n - 1;
  while (lo <= hi) {
    I32 mid = (lo + hi) / 2;
    if (a[mid] == key) return mid;
    if (a[mid] < key) lo = mid + 1; else hi = mid - 1;
  }
  return -1;
}
I64 __kai_run_15(void) {
  I32 a[64];
  U32 seed = 12345;
  for (I32 i = 0; i < 64; ++i) { seed = seed * 1103515245u + 12345u; a[i] = (I32)(seed >> 16) % 1000; }
  a[17] = 777;
  sort(a, 64);
  for (I32 i = 1; i < 64; ++i) if (a[i - 1] > a[i]) return 0;
  return find(a, 64, 777) >= 0;
}
";

const ENTRY_HASH: &str = "#define BUCKETS 64
typedef struct Slot { U32 key; I32 value; Bool used; } Slot;
static Slot slots[BUCKETS * 8];
static U32 hash(U32 k) { k ^= k >> 16; k *= 0x45d9f3bu; k ^= k >> 16; return k; }
static void put(U32 key, I32 value) {
  U32 i = hash(key) % (BUCKETS * 8);
  while (slots[i].used && slots[i].key != key) i = (i + 1) % (BUCKETS * 8);
  slots[i].key = key;
  slots[i].value = value;
  slots[i].used = TRUE;
}
static I32 get(U32 key) {
  U32 i = hash(key) % (BUCKETS * 8);
  while (slots[i].used) {
    if (slots[i].key == key) return slots[i].value;
    i = (i + 1) % (BUCKETS * 8);
  }
  return -1;
}
I64 __kai_run_16(void) {
  for (U32 k = 0; k < 300; ++k) put(k * 7919u, 1);
  I64 found = 0;
  for (U32 k = 0; k < 300; ++k) found += get(k * 7919u);
  return found;
}
";

const ENTRY_CALC: &str = "typedef struct { const char *p; } Lexer;
static I64 expr(Lexer *lx);
static void skip(Lexer *lx) { while (*lx->p == ' ') ++lx->p; }
static I64 number(Lexer *lx) {
  I64 v = 0;
  skip(lx);
  while (*lx->p >= '0' && *lx->p <= '9') v = v * 10 + (*lx->p++ - '0');
  return v;
}
static I64 atom(Lexer *lx) {
  skip(lx);
  if (*lx->p == '(') {
    ++lx->p;
    I64 v = expr(lx);
    skip(lx);
    if (*lx->p == ')') ++lx->p;
    return v;
  }
  if (*lx->p == '-') { ++lx->p; return -atom(lx); }
  return number(lx);
}
static I64 term(Lexer *lx) {
  I64 v = atom(lx);
  for (;;) {
    skip(lx);
    char c = *lx->p;
    if (c == '*') { ++lx->p; v *= atom(lx); }
    else if (c == '/') { ++lx->p; I64 d = atom(lx); v = d ? v / d : 0; }
    else if (c == '%') { ++lx->p; I64 d = atom(lx); v = d ? v % d : 0; }
    else return v;
  }
}
static I64 expr(Lexer *lx) {
  I64 v = term(lx);
  for (;;) {
    skip(lx);
    char c = *lx->p;
    if (c == '+') { ++lx->p; v += term(lx); }
    else if (c == '-') { ++lx->p; v -= term(lx); }
    else return v;
  }
}
I64 calc(const char *s) { Lexer lx = {s}; return expr(&lx); }
I64 __kai_run_17(void) { return calc(\"3 + 4 * (5 + 6) - 10 / 5 + 2\"); }
";

const ENTRY_BITS: &str =
    "static I32 popcount(U64 v) { I32 n = 0; while (v) { v &= v - 1; ++n; } return n; }
static U64 rotl(U64 v, I32 s) { return (v << s) | (v >> (64 - s)); }
I64 __kai_run_18(void) {
  U64 v = 0xF0F0F0F0F0F0F0F0ull;
  v = rotl(v, 4);
  return popcount(v);
}
";

const ENTRY_USES: &str = "I32 sq(I32 x);
extern I64 counter;
I64 fib(I64 n);
I64 __kai_run_20(void) { return sq(7) + counter + fib(20); }
";

/// The `notes` component's logic (components/notes/src/lib.rs) in C, with fixed
/// arrays instead of heap strings: add, count, find, get and the request switch.
const ENTRY_NOTES: &str = r#"#define MAX_NOTES 32
#define MAX_TEXT 96
#define MAX_REPLY 160
typedef struct Note {
  U32 id;
  U32 words;
  char text[MAX_TEXT];
} Note;
typedef struct Notes {
  Note notes[MAX_NOTES];
  I32 len;
  U32 next_id;
} Notes;
static Notes state = {{{0}}, 0, 1};
static char reply[MAX_REPLY];
static I32 reply_len;

static Bool is_space(char c) { return c == ' ' || c == '\t' || c == '\n'; }
static char lower(char c) { return c >= 'A' && c <= 'Z' ? c - 'A' + 'a' : c; }
static U32 words(const char *s) {
  U32 n = 0;
  Bool in = FALSE;
  for (; *s; ++s) {
    if (is_space(*s)) in = FALSE;
    else if (!in) { in = TRUE; ++n; }
  }
  return n;
}
static void say(const char *s) {
  while (*s && reply_len < MAX_REPLY - 1) reply[reply_len++] = *s++;
  reply[reply_len] = 0;
}
static void say_u32(U32 v) {
  char tmp[12];
  I32 n = 0;
  do { tmp[n++] = '0' + v % 10; v /= 10; } while (v);
  while (n > 0 && reply_len < MAX_REPLY - 1) reply[reply_len++] = tmp[--n];
  reply[reply_len] = 0;
}
static U32 add(const char *text) {
  if (state.len == MAX_NOTES) return 0;
  Note *note = &state.notes[state.len++];
  note->id = state.next_id++;
  note->words = words(text);
  I32 i = 0;
  for (; text[i] && i < MAX_TEXT - 1; ++i) note->text[i] = text[i];
  note->text[i] = 0;
  return note->id;
}
static Bool has_word(const char *text, const char *word) {
  while (*text) {
    while (is_space(*text)) ++text;
    const char *w = word;
    const char *t = text;
    while (*w && *t && !is_space(*t) && lower(*w) == lower(*t)) { ++w; ++t; }
    if (!*w && (!*t || is_space(*t))) return TRUE;
    while (*text && !is_space(*text)) ++text;
  }
  return FALSE;
}
static U32 parse_u32(const char *s, Bool *ok) {
  U32 v = 0;
  *ok = *s != 0;
  for (; *s; ++s) {
    if (*s < '0' || *s > '9') { *ok = FALSE; return 0; }
    v = v * 10 + (*s - '0');
  }
  return v;
}
static Bool starts(const char *s, const char *p) {
  while (*p) if (*s++ != *p++) return FALSE;
  return TRUE;
}
const char *handle(const char *request) {
  reply_len = 0;
  reply[0] = 0;
  if (starts(request, "add ")) {
    U32 id = add(request + 4);
    if (!id) { say("err notes are full"); return reply; }
    say("ok "); say_u32(id);
  } else if (starts(request, "count")) {
    U32 total = 0;
    for (I32 i = 0; i < state.len; ++i) total += state.notes[i].words;
    say("ok "); say_u32(state.len); say(" notes, "); say_u32(total); say(" words");
  } else if (starts(request, "find ")) {
    Bool any = FALSE;
    say("ok");
    for (I32 i = 0; i < state.len; ++i) {
      if (has_word(state.notes[i].text, request + 5)) { say(" "); say_u32(state.notes[i].id); any = TRUE; }
    }
    if (!any) { reply_len = 0; say("ok none"); }
  } else if (starts(request, "get ")) {
    Bool ok;
    U32 id = parse_u32(request + 4, &ok);
    for (I32 i = 0; ok && i < state.len; ++i) {
      if (state.notes[i].id == id) { say("ok "); say(state.notes[i].text); return reply; }
    }
    say("err no such note");
  } else {
    say("err unknown request");
  }
  return reply;
}
static Bool same(const char *a, const char *b) {
  while (*a && *a == *b) { ++a; ++b; }
  return *a == *b;
}
I64 __kai_run_19(void) {
  if (!same(handle("add buy milk"), "ok 1")) return 0;
  if (!same(handle("add call the Bank about milk"), "ok 2")) return 0;
  if (!same(handle("count"), "ok 2 notes, 7 words")) return 0;
  if (!same(handle("find MILK"), "ok 1 2")) return 0;
  if (!same(handle("find bank"), "ok 2")) return 0;
  if (!same(handle("find cheese"), "ok none")) return 0;
  if (!same(handle("get 2"), "ok call the Bank about milk")) return 0;
  if (!same(handle("get 9"), "err no such note")) return 0;
  Print(handle("count"));
  Print("\n");
  return 1;
}
"#;

/// Primes below 1,000,000 by the sieve; returns their count, 78,498.
pub const SIEVE: &str = "static U8 composite[1000001];
I64 sieve(I32 n) {
  I64 count = 0;
  for (I32 i = 2; i <= n; ++i) {
    if (composite[i]) continue;
    ++count;
    for (I32 j = i + i; j <= n; j += i) composite[j] = 1;
  }
  return count;
}
I64 __kai_run_sieve(void) { return sieve(1000000); }
";
pub const SIEVE_PRIMES: i64 = 78_498;

/// A 10-million-step sum.
pub const SUM: &str = "I64 sum(I64 n) {
  I64 s = 0;
  for (I64 i = 0; i < n; ++i) s += i ^ (i >> 3);
  return s;
}
I64 __kai_run_sum(void) { return sum(10000000); }
";

/// The sum's value, computed the same way.
pub fn sum_expected(n: i64) -> i64 {
    (0..n).fold(0i64, |s, i| s.wrapping_add(i ^ (i >> 3)))
}

/// The same loops in Rhai (no `while` in strict Rhai, but the spike's engine is
/// plain Rhai, so `for` over ranges and arrays).
pub const RHAI_SIEVE: &str = "let n = 1000000;
let composite = blob(n + 1, 0);
let count = 0;
for i in 2..=n {
  if composite[i] != 0 { continue; }
  count += 1;
  let j = i + i;
  while j <= n { composite[j] = 1; j += i; }
}
count
";
pub const RHAI_SUM: &str = "let s = 0;
for i in 0..10000000 { s += i ^ (i >> 3); }
s
";

/// Measure (g): a redefinition switches an existing caller and a stored pointer.
/// Each step is (source, run, expected).
pub const REDEFINE: &[(&str, Option<(&str, i64)>)] = &[
    ("I32 twice(I32 x) { return 2 * x; }\n", None),
    (
        "I32 twice(I32 x);\nI32 (*saved)(I32) = twice;\nI32 call_twice(I32 x) { return twice(x); }\n",
        None,
    ),
    (
        "I32 call_twice(I32 x);\nextern I32 (*saved)(I32);\nI64 __kai_run_r1(void) { return call_twice(5) * 100 + saved(5); }\n",
        Some(("__kai_run_r1", 1010)),
    ),
    ("I32 twice(I32 x) { return 3 * x; }\n", None),
    (
        "I32 call_twice(I32 x);\nextern I32 (*saved)(I32);\nI64 __kai_run_r2(void) { return call_twice(5) * 100 + saved(5); }\n",
        Some(("__kai_run_r2", 1515)),
    ),
];
