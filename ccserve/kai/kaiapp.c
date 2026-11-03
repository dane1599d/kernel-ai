/* The runtime `cc build` links into an app that includes <kaiapp.h> (plan
 * "Kai builds apps", piece 2): the `window` world's exports, around the app's
 * init, on_event and view. view's ui_ calls fill a frame; the runtime sends
 * the window only what changed since the frame it last showed, and when the
 * tree's shape changed it sends the whole tree again. */

#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <window.h>
#include <kaiapp.h>

/* The window's size in logical px. */
#define WIDTH 480
#define HEIGHT 320
/* Bytes of text one frame holds, NULs included. */
#define FRAME_TEXT 8192
/* Most bytes one text keeps. */
#define TEXT_MAX 256
/* Space between a row's or column's children, in logical px. */
#define GAP 8
/* A changed field's text, NUL included. */
#define EVENT_TEXT 1024

enum { COLUMN, ROW, LABEL, HEADING, BUTTON, FIELD, TOGGLE };

struct node {
    unsigned id, parent;
    unsigned char kind, on;
    /* Offsets into the frame's text. */
    unsigned short text, text2;
};

struct frame {
    int count;
    int used;
    struct node nodes[KAI_NODES];
    /* Its last byte stays 0: the text of a node that did not fit. */
    char text[FRAME_TEXT + 1];
};

static I64 state[KAI_STATE_MAX / sizeof(I64)];
static struct frame frames[2];
static int shown_at;
static int building;
static int is_open;
static kai_ui_types_window_id_t window_id;
static char event_text[EVENT_TEXT];

static struct state *the_state(void) {
    return (struct state *)state;
}

static void say(const char *text) {
    window_string_t line;
    window_string_set(&line, text);
    kai_app_log_write(&line);
}

static unsigned short keep(struct frame *f, const char *text) {
    size_t n = strlen(text);
    if (n > TEXT_MAX)
        n = TEXT_MAX;
    if (f->used + (int)n + 1 > FRAME_TEXT)
        return FRAME_TEXT;
    unsigned short at = (unsigned short)f->used;
    memcpy(f->text + at, text, n);
    f->text[at + n] = 0;
    f->used += (int)n + 1;
    return at;
}

static const struct node *find(const struct frame *f, unsigned id) {
    for (int i = 0; i < f->count; i++)
        if (f->nodes[i].id == id)
            return &f->nodes[i];
    return NULL;
}

/* One node of the frame view is building. A bad id, a repeated one, a
 * parent that is not a row or column put before it, or one node too many is
 * left out and logged. */
static void add(int kind, I64 id, I64 parent, const char *text, const char *text2, I64 on) {
    struct frame *f = &frames[1 - shown_at];
    if (!building)
        return;
    const struct node *up = parent == 0 ? NULL : find(f, (unsigned)parent);
    int container = parent == 0 || (up != NULL && up->kind <= ROW);
    if (id < 1 || id > KAI_NODES || parent < 0 || parent > KAI_NODES || !container ||
        find(f, (unsigned)id) != NULL || f->count == KAI_NODES) {
        say("kaiapp: a ui_ call was left out: a bad node or parent");
        return;
    }
    struct node *n = &f->nodes[f->count++];
    n->id = (unsigned)id;
    n->parent = (unsigned)parent;
    n->kind = (unsigned char)kind;
    n->on = on != 0;
    n->text = keep(f, text ? text : "");
    n->text2 = keep(f, text2 ? text2 : "");
}

void ui_column(I64 node, I64 parent) { add(COLUMN, node, parent, NULL, NULL, 0); }
void ui_row(I64 node, I64 parent) { add(ROW, node, parent, NULL, NULL, 0); }
void ui_label(I64 node, I64 parent, const char *text) { add(LABEL, node, parent, text, NULL, 0); }
void ui_heading(I64 node, I64 parent, const char *text) { add(HEADING, node, parent, text, NULL, 0); }
void ui_button(I64 node, I64 parent, const char *text) { add(BUTTON, node, parent, text, NULL, 0); }
void ui_field(I64 node, I64 parent, const char *label, const char *value) {
    add(FIELD, node, parent, label, value, 0);
}
void ui_toggle(I64 node, I64 parent, const char *label, I64 on) {
    add(TOGGLE, node, parent, label, NULL, on);
}

static int same(const struct frame *a, const struct node *x, const struct frame *b,
                const struct node *y) {
    return x->on == y->on && strcmp(a->text + x->text, b->text + y->text) == 0 &&
           strcmp(a->text + x->text2, b->text + y->text2) == 0;
}

static int put(const struct frame *f, const struct node *n) {
    kai_ui_types_layout_t layout;
    memset(&layout, 0, sizeof layout);
    layout.width.tag = KAI_UI_TYPES_SIZE_FIT;
    layout.height.tag = KAI_UI_TYPES_SIZE_FIT;
    kai_ui_types_widget_t w;
    memset(&w, 0, sizeof w);
    window_string_t text, text2;
    window_string_set(&text, f->text + n->text);
    window_string_set(&text2, f->text + n->text2);
    switch (n->kind) {
    case COLUMN:
    case ROW:
        layout.width.tag = KAI_UI_TYPES_SIZE_FILL;
        layout.width.val.fill = 1;
        layout.gap = GAP;
        w.tag = KAI_UI_TYPES_WIDGET_STACK;
        w.val.stack.axis = n->kind == ROW ? KAI_UI_TYPES_AXIS_ROW : KAI_UI_TYPES_AXIS_COLUMN;
        break;
    case LABEL:
    case HEADING:
        w.tag = KAI_UI_TYPES_WIDGET_LABEL;
        w.val.label.text = text;
        w.val.label.style =
            n->kind == HEADING ? KAI_UI_TYPES_TEXT_STYLE_HEADING : KAI_UI_TYPES_TEXT_STYLE_BODY;
        w.val.label.tone = KAI_UI_TYPES_TONE_NORMAL;
        break;
    case BUTTON:
        w.tag = KAI_UI_TYPES_WIDGET_BUTTON;
        w.val.button.text = text;
        break;
    case FIELD:
        w.tag = KAI_UI_TYPES_WIDGET_TEXT_LINE;
        w.val.text_line.label = text;
        w.val.text_line.value = text2;
        w.val.text_line.max_len = EVENT_TEXT - 1;
        break;
    default:
        w.tag = KAI_UI_TYPES_WIDGET_TOGGLE;
        w.val.toggle.label = text;
        w.val.toggle.on = n->on;
        break;
    }
    kai_ui_types_ui_error_t err;
    return kai_ui_view_put(window_id, n->id, n->parent, &layout, &w, &err);
}

/* Sends the frame view makes now, and commits. A refused call closes the
 * window, so the next render opens it again with the whole tree. */
static void render(void) {
    kai_ui_types_ui_error_t err;
    struct frame *shown = &frames[shown_at];
    struct frame *next = &frames[1 - shown_at];
    if (!is_open) {
        window_string_t title;
        window_string_set(&title, TITLE);
        if (!kai_ui_view_open(&title, WIDTH, HEIGHT, &window_id, &err)) {
            say("kaiapp: the window did not open");
            return;
        }
        is_open = 1;
        shown->count = 0;
    }
    next->count = 0;
    next->used = 0;
    building = 1;
    view(the_state());
    building = 0;
    int whole = shown->count != next->count;
    for (int i = 0; !whole && i < next->count; i++) {
        const struct node *a = &shown->nodes[i], *b = &next->nodes[i];
        whole = a->id != b->id || a->parent != b->parent || a->kind != b->kind;
    }
    int ok = 1;
    if (whole) {
        for (int i = 0; i < shown->count; i++)
            if (shown->nodes[i].parent == 0)
                ok &= kai_ui_view_remove(window_id, shown->nodes[i].id, &err);
        for (int i = 0; i < next->count; i++)
            ok &= put(next, &next->nodes[i]);
    } else {
        for (int i = 0; i < next->count; i++)
            if (!same(shown, &shown->nodes[i], next, &next->nodes[i]))
                ok &= put(next, &next->nodes[i]);
    }
    ok &= kai_ui_view_commit(window_id, &err);
    shown_at = 1 - shown_at;
    if (!ok) {
        say("kaiapp: the window refused a change; it opens again");
        kai_ui_view_close(window_id, &err);
        is_open = 0;
    }
}

uint32_t exports_kai_live_lifecycle_schema(void) { return 1; }

bool exports_kai_live_lifecycle_init(exports_kai_live_lifecycle_start_error_t *err) {
    memset(state, 0, sizeof state);
    init(the_state());
    render();
    return true;
}

bool exports_kai_live_lifecycle_migrate(window_list_u8_t *saved, uint32_t from_schema,
                                        exports_kai_live_lifecycle_start_error_t *err) {
    int fits = saved->len <= sizeof state;
    if (from_schema == 1 && fits) {
        memset(state, 0, sizeof state);
        memcpy(state, saved->ptr, saved->len);
    }
    window_list_u8_free(saved);
    if (from_schema != 1) {
        *err = EXPORTS_KAI_LIVE_LIFECYCLE_START_ERROR_UNSUPPORTED_SCHEMA;
        return false;
    }
    if (!fits) {
        *err = EXPORTS_KAI_LIVE_LIFECYCLE_START_ERROR_BAD_STATE;
        return false;
    }
    render();
    return true;
}

bool exports_kai_live_lifecycle_save(uint32_t to_schema, window_list_u8_t *ret) {
    ret->ptr = NULL;
    ret->len = 0;
    if (to_schema != 0 && to_schema != 1)
        return false;
    ret->ptr = malloc(sizeof state);
    if (ret->ptr == NULL)
        return false;
    memcpy(ret->ptr, state, sizeof state);
    ret->len = sizeof state;
    return true;
}

uint32_t exports_kai_live_lifecycle_health(void) { return 0; }
uint32_t exports_kai_live_lifecycle_self_test(void) { return 0; }

void exports_kai_app_events_on_event(exports_kai_app_events_event_t *e) {
    exports_kai_app_events_event_free(e);
}

void exports_kai_ui_input_on_ui(exports_kai_ui_input_window_id_t window,
                                exports_kai_ui_input_ui_event_t *e) {
    struct kai_event event = {0, 0, 0, ""};
    switch (e->tag) {
    case EXPORTS_KAI_UI_INPUT_UI_EVENT_PRESSED:
        event.kind = KAI_PRESSED;
        event.node = e->val.pressed;
        break;
    case EXPORTS_KAI_UI_INPUT_UI_EVENT_SUBMITTED:
        event.kind = KAI_SUBMITTED;
        event.node = e->val.submitted;
        break;
    case EXPORTS_KAI_UI_INPUT_UI_EVENT_CHANGED: {
        exports_kai_ui_input_value_t *v = &e->val.changed.f1;
        event.kind = KAI_CHANGED;
        event.node = e->val.changed.f0;
        if (v->tag == EXPORTS_KAI_UI_INPUT_VALUE_TEXT) {
            size_t n = v->val.text.len < EVENT_TEXT - 1 ? v->val.text.len : EVENT_TEXT - 1;
            memcpy(event_text, v->val.text.ptr, n);
            event_text[n] = 0;
            event.text = event_text;
        } else if (v->tag == EXPORTS_KAI_UI_INPUT_VALUE_ON) {
            event.number = v->val.on;
        } else if (v->tag == EXPORTS_KAI_UI_INPUT_VALUE_NUMBER) {
            event.number = v->val.number;
        } else {
            event.number = v->val.index;
        }
        break;
    }
    default: {
        kai_ui_types_ui_error_t err;
        if (is_open && window == window_id) {
            kai_ui_view_close(window_id, &err);
            is_open = 0;
        }
        exports_kai_ui_input_ui_event_free(e);
        return;
    }
    }
    exports_kai_ui_input_ui_event_free(e);
    if (!is_open || window != window_id)
        return;
    on_event(the_state(), &event);
    render();
}

static void emit(char *out, I64 cap, I64 *at, char c) {
    if (*at + 1 < cap)
        out[*at] = c;
    (*at)++;
}

I64 Format(char *out, I64 cap, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    I64 at = 0;
    for (const char *p = fmt; *p; p++) {
        if (*p != '%') {
            emit(out, cap, &at, *p);
            continue;
        }
        p++;
        if (p[0] == 'l' && p[1] == 'l' && p[2] == 'd') {
            p += 2;
            I64 v = va_arg(ap, I64);
            unsigned long long u = v < 0 ? 0ull - (unsigned long long)v : (unsigned long long)v;
            char digits[20];
            int n = 0;
            do {
                digits[n++] = (char)('0' + u % 10);
                u /= 10;
            } while (u);
            if (v < 0)
                emit(out, cap, &at, '-');
            while (n)
                emit(out, cap, &at, digits[--n]);
        } else if (*p == 's') {
            const char *s = va_arg(ap, const char *);
            for (s = s ? s : ""; *s; s++)
                emit(out, cap, &at, *s);
        } else if (*p == 'c') {
            emit(out, cap, &at, (char)va_arg(ap, int));
        } else if (*p == '%') {
            emit(out, cap, &at, '%');
        } else {
            break;
        }
    }
    va_end(ap);
    if (cap <= 0)
        return 0;
    out[at < cap ? at : cap - 1] = 0;
    return at < cap ? at : cap - 1;
}
