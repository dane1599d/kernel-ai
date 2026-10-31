/* The runtime `cc build` links into every component and app (ADR 0012 D7):
 * the allocator behind <stdlib.h>, which wit-bindgen's glue calls, abort, and
 * <string.h>'s common calls. It imports nothing but the world's
 * kai:app/log panic, through the glue.
 *
 * The heap starts where the module's memory ends on first use and grows by
 * memory.grow. Free blocks are kept in one list in address order, and a freed
 * block merges with free neighbours, so memory is reused. C is memory-unsafe
 * inside its own instance (D5): a bad free corrupts only this heap. */

#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define ALIGN 8u
#define PAGE 65536u
/* Pages in a 4 GiB memory, where the heap's addresses would wrap. */
#define MAX_PAGES 65536u
/* Largest request: anything bigger fails before sizes can wrap. */
#define MAX_SIZE 0x40000000u

typedef struct Block {
    /* Payload bytes after this header, a multiple of ALIGN. */
    size_t size;
    /* The next free block, while this one is free. */
    struct Block *next;
} Block;

static Block *free_list;
/* The heap's end, and the first byte at its end never handed out. */
static uintptr_t heap_end;
static uintptr_t heap_top;

static size_t round_up(size_t n) {
    return (n + ALIGN - 1) & ~(size_t)(ALIGN - 1);
}

/* Makes at least `need` bytes free at heap_top, in whole pages. Memory that
 * grew by other means since lies past heap_end, so the heap starts again at
 * memory's end and never hands that out. */
static int grow(size_t need) {
    size_t now = (size_t)__builtin_wasm_memory_size(0);
    if (now == 0 || now >= MAX_PAGES)
        return 0;
    if ((uintptr_t)now * PAGE != heap_end) {
        heap_end = (uintptr_t)now * PAGE;
        heap_top = heap_end;
    }
    if (heap_end - heap_top >= need)
        return 1;
    size_t pages = (need - (heap_end - heap_top) + PAGE - 1) / PAGE;
    if (pages >= MAX_PAGES - now)
        return 0;
    if (__builtin_wasm_memory_grow(0, pages) != (int)now)
        return 0;
    heap_end += (uintptr_t)pages * PAGE;
    return 1;
}

void *malloc(size_t size) {
    if (size > MAX_SIZE)
        return NULL;
    size_t need = round_up(size == 0 ? 1 : size);
    Block **link = &free_list;
    for (Block *b = free_list; b != NULL; link = &b->next, b = b->next) {
        if (b->size < need)
            continue;
        if (b->size - need >= sizeof(Block) + ALIGN) {
            Block *rest = (Block *)((char *)(b + 1) + need);
            rest->size = b->size - need - sizeof(Block);
            rest->next = b->next;
            *link = rest;
            b->size = need;
        } else {
            *link = b->next;
        }
        return b + 1;
    }
    if (!grow(sizeof(Block) + need))
        return NULL;
    Block *b = (Block *)heap_top;
    heap_top += sizeof(Block) + need;
    b->size = need;
    return b + 1;
}

void free(void *ptr) {
    if (ptr == NULL)
        return;
    Block *b = (Block *)ptr - 1;
    Block *prev = NULL;
    Block *next = free_list;
    while (next != NULL && next < b) {
        prev = next;
        next = next->next;
    }
    if (next != NULL && (char *)(b + 1) + b->size == (char *)next) {
        b->size += sizeof(Block) + next->size;
        b->next = next->next;
    } else {
        b->next = next;
    }
    if (prev == NULL) {
        free_list = b;
    } else if ((char *)(prev + 1) + prev->size == (char *)b) {
        prev->size += sizeof(Block) + b->size;
        prev->next = b->next;
    } else {
        prev->next = b;
    }
}

void *calloc(size_t count, size_t size) {
    if (size != 0 && count > MAX_SIZE / size)
        return NULL;
    void *ptr = malloc(count * size);
    if (ptr != NULL)
        memset(ptr, 0, count * size);
    return ptr;
}

void *realloc(void *ptr, size_t size) {
    if (ptr == NULL)
        return malloc(size);
    Block *b = (Block *)ptr - 1;
    if (size <= b->size)
        return ptr;
    void *out = malloc(size);
    if (out != NULL) {
        memcpy(out, ptr, b->size);
        free(ptr);
    }
    return out;
}

/* Both worlds' glue defines this wrapper for kai:app/log panic, which names
 * where a trap that follows came from. */
struct __kai_str {
    const char *ptr;
    size_t len;
};
void kai_app_log_panic(struct __kai_str *file, uint32_t line, uint32_t column);

/* `cc build` gives this function the body `unreachable`, the trap a panic
 * ends in, which wcc has no builtin for. Until then a division by zero traps. */
int __kai_trap(void) {
    volatile int zero = 0;
    return 1 / zero;
}

_Noreturn void abort(void) {
    struct __kai_str what = {"abort", 5};
    kai_app_log_panic(&what, 0, 0);
    __kai_trap();
    for (;;) {
    }
}

size_t strlen(const char *s) {
    size_t n = 0;
    while (s[n])
        n++;
    return n;
}

int strcmp(const char *a, const char *b) {
    while (*a && *a == *b) {
        a++;
        b++;
    }
    return (unsigned char)*a - (unsigned char)*b;
}

int strncmp(const char *a, const char *b, size_t n) {
    for (; n > 0; n--, a++, b++) {
        if (*a != *b || !*a)
            return (unsigned char)*a - (unsigned char)*b;
    }
    return 0;
}

char *strcpy(char *dst, const char *src) {
    char *d = dst;
    while ((*d++ = *src++))
        ;
    return dst;
}

char *strchr(const char *s, int c) {
    for (;; s++) {
        if (*s == (char)c)
            return (char *)s;
        if (!*s)
            return NULL;
    }
}

void *memcpy(void *dst, const void *src, size_t n) {
    char *d = dst;
    const char *s = src;
    while (n--)
        *d++ = *s++;
    return dst;
}

void *memmove(void *dst, const void *src, size_t n) {
    char *d = dst;
    const char *s = src;
    if (d < s) {
        while (n--)
            *d++ = *s++;
    } else {
        while (n--)
            d[n] = s[n];
    }
    return dst;
}

void *memset(void *dst, int c, size_t n) {
    char *d = dst;
    while (n--)
        *d++ = (char)c;
    return dst;
}

int memcmp(const void *a, const void *b, size_t n) {
    const unsigned char *x = a, *y = b;
    for (; n > 0; n--, x++, y++) {
        if (*x != *y)
            return *x - *y;
    }
    return 0;
}
