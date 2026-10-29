/* kai.h's fixed part (ADR 0012 D1): HolyC's type names, the calls that show
 * a value, and kairt's Print. The service wraps it in its include guard and
 * adds the OS's actions after it (ccserve/src/rows), so this file has no guard
 * of its own. */
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
#ifndef NULL
#define NULL ((void *)0)
#endif
#define __KAI_IMPORT(field) __attribute__((import_module("kai"), import_name(field)))
__KAI_IMPORT("__show_i64") void __kai_show_i64(I64 value);
__KAI_IMPORT("__show_u64") void __kai_show_u64(U64 value);
__KAI_IMPORT("__show_f64") void __kai_show_f64(F64 value);
__KAI_IMPORT("__show_str") void __kai_show_str(const char *value);
__KAI_IMPORT("__show_ptr") void __kai_show_ptr(const void *value);
#define __kai_show(x) _Generic((x), \
    char *: __kai_show_str, const char *: __kai_show_str, \
    void *: __kai_show_ptr, const void *: __kai_show_ptr, \
    float: __kai_show_f64, double: __kai_show_f64, \
    unsigned char: __kai_show_u64, unsigned short: __kai_show_u64, \
    unsigned int: __kai_show_u64, unsigned long: __kai_show_u64, \
    unsigned long long: __kai_show_u64, \
    default: __kai_show_i64)(x)
I64 Print(const char *fmt, ...);
