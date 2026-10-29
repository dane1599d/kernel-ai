/* kairt: the runtime linked into every session first (ADR 0012 D2), compiled
 * by cc.wasm when `cc` starts. Print and the string functions of <string.h>
 * that sessions use most. Output goes to fd 1 through WASI fd_write, the only
 * WASI call session code may import; `cc` keeps it up to its output limit.
 *
 * Print follows C's printf: %d reads an int, %lld a long long. wcc passes each
 * variadic argument in an 8-byte slot, so an I64 printed with %d shows its low
 * 32 bits. */

#include <kai.h>
#include <stdarg.h>
#include <string.h>

__attribute__((import_module("wasi_snapshot_preview1"), import_name("fd_write")))
int __kai_fd_write(int fd, const void *iovs, int count, int *written);

struct __kai_iov {
    const char *base;
    unsigned int len;
};

typedef struct {
    char buf[256];
    int len;
    I64 total;
} __KaiOut;

static void out_flush(__KaiOut *o) {
    if (o->len > 0) {
        struct __kai_iov iov;
        int written;
        iov.base = o->buf;
        iov.len = (unsigned int)o->len;
        __kai_fd_write(1, &iov, 1, &written);
        o->len = 0;
    }
}

static void out_char(__KaiOut *o, char c) {
    if (o->len == (int)sizeof(o->buf))
        out_flush(o);
    o->buf[o->len++] = c;
    o->total++;
}

static void out_pad(__KaiOut *o, int n, char c) {
    for (; n > 0; n--)
        out_char(o, c);
}

/* A field of `n` bytes from `s`, padded to `width`. */
static void out_field(__KaiOut *o, const char *s, int n, int width, int left, char pad) {
    if (!left)
        out_pad(o, width - n, pad);
    for (int i = 0; i < n; i++)
        out_char(o, s[i]);
    if (left)
        out_pad(o, width - n, ' ');
}

/* `v` in `base`, with a sign, into a field. */
static void out_number(__KaiOut *o, U64 v, int negative, int base, int upper, int width,
                       int left, int zero, char sign) {
    char digits[24];
    const char *set = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    int n = 0;
    do {
        digits[n++] = set[v % (U64)base];
        v /= (U64)base;
    } while (v != 0);
    char s[26];
    int len = 0;
    if (negative)
        s[len++] = '-';
    else if (sign)
        s[len++] = sign;
    int signs = len;
    while (n > 0)
        s[len++] = digits[--n];
    /* Zero padding goes between the sign and the digits. */
    if (zero && !left) {
        for (int i = 0; i < signs; i++)
            out_char(o, s[i]);
        out_field(o, s + signs, len - signs, width - signs, 0, '0');
        return;
    }
    out_field(o, s, len, width, left, ' ');
}

/* `v` with `precision` decimals; no exponent form. */
static void out_float(__KaiOut *o, F64 v, int precision, int width, int left, char sign) {
    char s[48];
    int len = 0;
    if (v != v) {
        out_field(o, "nan", 3, width, left, ' ');
        return;
    }
    if (v < 0) {
        s[len++] = '-';
        v = -v;
    } else if (sign) {
        s[len++] = sign;
    }
    if (v > 1e18) {
        s[len++] = 'i';
        s[len++] = 'n';
        s[len++] = 'f';
        out_field(o, s, len, width, left, ' ');
        return;
    }
    if (precision > 9)
        precision = 9;
    U64 scale = 1;
    for (int i = 0; i < precision; i++)
        scale *= 10;
    U64 whole = (U64)v;
    U64 frac = (U64)((v - (F64)whole) * (F64)scale + 0.5);
    if (frac >= scale) {
        whole++;
        frac -= scale;
    }
    char digits[24];
    int n = 0;
    do {
        digits[n++] = (char)('0' + whole % 10);
        whole /= 10;
    } while (whole != 0);
    while (n > 0)
        s[len++] = digits[--n];
    if (precision > 0) {
        s[len++] = '.';
        for (int i = precision - 1; i >= 0; i--) {
            s[len + i] = (char)('0' + frac % 10);
            frac /= 10;
        }
        len += precision;
    }
    out_field(o, s, len, width, left, ' ');
}

I64 __kai_vprint(const char *fmt, va_list ap) {
    __KaiOut o;
    o.len = 0;
    o.total = 0;
    for (const char *p = fmt; *p; p++) {
        if (*p != '%') {
            out_char(&o, *p);
            continue;
        }
        int left = 0, zero = 0, width = 0, precision = -1, size = 0;
        char sign = 0;
        for (;; p++) {
            if (p[1] == '-')
                left = 1;
            else if (p[1] == '0')
                zero = 1;
            else if (p[1] == '+')
                sign = '+';
            else if (p[1] == ' ')
                sign = sign ? sign : ' ';
            else
                break;
        }
        if (p[1] == '*') {
            width = va_arg(ap, int);
            p++;
        }
        while (p[1] >= '0' && p[1] <= '9')
            width = width * 10 + (*++p - '0');
        if (p[1] == '.') {
            p++;
            precision = 0;
            if (p[1] == '*') {
                precision = va_arg(ap, int);
                p++;
            }
            while (p[1] >= '0' && p[1] <= '9')
                precision = precision * 10 + (*++p - '0');
        }
        while (p[1] == 'l' || p[1] == 'h' || p[1] == 'z' || p[1] == 'j' || p[1] == 't') {
            if (p[1] == 'l' || p[1] == 'j')
                size++;
            p++;
        }
        if (width > 1024)
            width = 1024;
        char c = *++p;
        I64 v;
        U64 u;
        switch (c) {
        case 'd':
        case 'i':
            v = size >= 2 ? va_arg(ap, I64) : (I64)va_arg(ap, int);
            u = v < 0 ? (U64)0 - (U64)v : (U64)v;
            out_number(&o, u, v < 0, 10, 0, width, left, zero, sign);
            break;
        case 'u':
        case 'x':
        case 'X':
        case 'o':
            u = size >= 2 ? va_arg(ap, U64) : (U64)va_arg(ap, unsigned int);
            out_number(&o, u, 0, c == 'u' ? 10 : c == 'o' ? 8 : 16, c == 'X', width, left,
                       zero, 0);
            break;
        case 'p':
            out_char(&o, '0');
            out_char(&o, 'x');
            out_number(&o, (U64)(unsigned long)va_arg(ap, void *), 0, 16, 0, 0, 0, 0, 0);
            break;
        case 'c': {
            char ch = (char)va_arg(ap, int);
            out_field(&o, &ch, 1, width, left, ' ');
            break;
        }
        case 's': {
            const char *s = va_arg(ap, const char *);
            if (!s)
                s = "(null)";
            int n = 0;
            while (s[n] && (precision < 0 || n < precision))
                n++;
            out_field(&o, s, n, width, left, ' ');
            break;
        }
        case 'f':
        case 'F':
        case 'e':
        case 'g':
            out_float(&o, va_arg(ap, F64), precision < 0 ? 6 : precision, width, left, sign);
            break;
        case '%':
            out_char(&o, '%');
            break;
        case 0:
            p--;
            break;
        default:
            out_char(&o, '%');
            out_char(&o, c);
            break;
        }
    }
    out_flush(&o);
    return o.total;
}

I64 Print(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    I64 n = __kai_vprint(fmt, ap);
    va_end(ap);
    return n;
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
