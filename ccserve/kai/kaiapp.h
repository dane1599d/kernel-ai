/* <kaiapp.h>: a window app in four parts (plan "Kai builds apps", piece 2).
 * Including it makes `cc build` build on the `window` world, and links the
 * runtime (kaiapp.c) that owns that world's exports. The app gives:
 *
 *   const char TITLE[] = "Counter";
 *   struct state { I64 count; };          at most KAI_STATE_MAX bytes, no pointers
 *   void init(struct state *s);           the first state; s starts zeroed
 *   void on_event(struct state *s, const struct kai_event *e);
 *   void view(const struct state *s);     the window, with the ui_ calls
 *
 * After init and after each event the runtime calls view, sends what changed
 * and commits. The state is saved as its bytes, so a restart keeps it; a
 * version whose struct state has another size starts afresh from init. */
#pragma once

#include <stddef.h>
#include <string.h>

typedef long long I64;

/* Most bytes struct state may take. */
#define KAI_STATE_MAX 4096
/* Most nodes view may make; node ids are 1 to this. */
#define KAI_NODES 128

/* What happened: a button was pressed, a field or toggle changed, or Enter
 * was pressed in a field. node is the node's id; a field's new text is in
 * text, a toggle's new value in number (1 on, 0 off). */
#define KAI_PRESSED 1
#define KAI_CHANGED 2
#define KAI_SUBMITTED 3
struct kai_event {
    I64 kind;
    I64 node;
    I64 number;
    const char *text;
};

struct state;
extern const char TITLE[];
/* sizeof(struct state): `cc build` defines it after the app's first file. */
extern const I64 kai_state_size;
void init(struct state *s);
void on_event(struct state *s, const struct kai_event *e);
void view(const struct state *s);

/* view's calls. parent 0 is the window, a column; put a parent before its
 * children. Text is copied. */
void ui_column(I64 node, I64 parent);
void ui_row(I64 node, I64 parent);
void ui_label(I64 node, I64 parent, const char *text);
void ui_heading(I64 node, I64 parent, const char *text);
void ui_button(I64 node, I64 parent, const char *text);
void ui_field(I64 node, I64 parent, const char *label, const char *value);
void ui_toggle(I64 node, I64 parent, const char *label, I64 on);

/* Writes fmt into out, at most cap bytes with its NUL: %lld, %s, %c, %%.
 * Returns the length written. */
I64 Format(char *out, I64 cap, const char *fmt, ...);

/* A random number from 0 to RAND_MAX: a fresh one each time, from the seed
 * the OS gives the call, so a call replayed on another version sees the same
 * ones. rand() % n is 0 to n - 1. It needs no srand. Declared as C's own, so
 * an app's `int rand(void);` agrees with it. */
#define RAND_MAX 2147483647
int rand(void);
