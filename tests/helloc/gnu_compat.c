// Shims for the glibc-only symbols in Ubuntu's libsupc++.a / libgcc_eh.a.
//
// gcc's C++ ABI runtime on a glibc distro is itself linked against glibc, so dropping it on top of
// musl leaves three dangling references. None of them needs glibc semantics -- each has a correct,
// well-defined answer -- so a ~30-line shim buys the whole C++ ABI without building libc++ from
// source (which would need cmake, absent here).
//
// This exists to answer the gate question "does the C++ ABI work on Nyx". If the real libc++ is
// built against musl later, all three disappear: libc++abi has no such dependencies.

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <stddef.h>

// ★ This file is fed to g++, which compiles a .c file AS C++ -- so without this every shim below
// would get C++ name mangling and satisfy nothing, while the linker kept reporting the exact same
// undefined symbols. The failure looks identical to the shims not being linked at all.
#ifdef __cplusplus
extern "C" {
#endif

// glibc 2.32+. libsupc++'s __cxa_guard_acquire reads it to skip atomics in a single-threaded
// program. **Deliberately 0 = "may be multithreaded"**, which forces the atomic path: that is
// the conservative answer, and it is the CORRECT one here because Nyx now has real threads via
// clone(2). Setting it to 1 would make guarded static initialisation racy the moment two threads
// touched the same function-local static.
char __libc_single_threaded = 0;

// glibc's _FORTIFY_SOURCE sprintf. `flag` is the fortify level and `slen` the compiler's idea of
// the destination size, or (size_t)-1 when it could not determine one. Forwarding to vsnprintf
// preserves the bounds check that the _chk name promises rather than quietly dropping it.
int __sprintf_chk(char *s, int flag, size_t slen, const char *fmt, ...) {
    (void)flag;
    va_list ap;
    va_start(ap, fmt);
    int n = (slen == (size_t)-1) ? vsprintf(s, fmt, ap)
                                 : vsnprintf(s, slen, fmt, ap);
    va_end(ap);
    return n;
}

// glibc 2.35+ fast path for _Unwind_Find_FDE. Returning "not found" is not a degradation: the
// unwinder then falls back to dl_iterate_phdr, which musl implements for static binaries out of
// AT_PHDR/AT_PHNUM -- auxv entries Nyx's build_initial_stack already provides. That fallback is
// the path we actually want exercised, since it is the one a musl-built libc++abi would use.
int _dl_find_object(void *address, void *result) {
    (void)address;
    (void)result;
    return -1;
}

// glibc 2.38+ redirects the C23-conformant string-to-number functions to __isoc23_* aliases at
// the header level, so anything compiled against those headers references the alias rather than
// the plain name. The C23 change only concerns binary literals ("0b1010" with base 0 or 2);
// forwarding to musl's implementation differs solely for that input, which the C++ demangler and
// eh_alloc's getenv parsing never produce.
unsigned long __isoc23_strtoul(const char *s, char **end, int base) { return strtoul(s, end, base); }
long          __isoc23_strtol (const char *s, char **end, int base) { return strtol(s, end, base); }

#ifdef __cplusplus
}
#endif
