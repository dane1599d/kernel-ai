/* <stdlib.h> for what `cc build` makes (ADR 0012 D7): the allocation calls
 * wit-bindgen's C output uses, and abort. xcc's own headers in /kai/include
 * have no stdlib.h; a component or app gets these from the allocator its
 * build links in. */
#pragma once

#include <stddef.h>

void *malloc(size_t size);
void *calloc(size_t count, size_t size);
void *realloc(void *ptr, size_t size);
void free(void *ptr);
_Noreturn void abort(void);
