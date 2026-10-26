#define MAX_NOTES 32
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
