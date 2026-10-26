#ifndef KAI_H
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
