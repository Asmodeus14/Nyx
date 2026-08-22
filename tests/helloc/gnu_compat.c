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
#include <stdint.h>
#include <elf.h>

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

// ★★★ _dl_find_object IS the unwinder's only lookup path -- there is NO fallback.
//
// This was a stub returning -1 ("not found"), on the belief that _Unwind_Find_FDE would then fall
// back to dl_iterate_phdr. IT DOES NOT. In gcc 12+ built against glibc 2.35+ headers,
// DLFO_STRUCT_HAS_EH_FRAME is defined and the dl_iterate_phdr walk is #if'd OUT of
// unwind-dw2-fde-dip.c entirely -- _dl_find_object REPLACES it rather than fronting it. Proof in
// the linked binary: _Unwind_Find_FDE calls _dl_find_object and `nm` finds no dl_iterate_phdr at
// all, because nothing references it so the linker never pulls musl's copy in. So the stub made
// every single throw fail to find a handler -> std::terminate -> abort, and musl's abort ends in
// a_crash() = `hlt`, which is privileged: it surfaced as a #GP inside abort() with nothing
// anywhere pointing at exceptions.
//
// Doing it properly is ~20 lines, because a static binary is the easy case: there is exactly one
// object, it is always the answer, and its program headers describe it completely.
//
// Contract (glibc): fill *result and return 0 if ADDRESS lies in an object, else return -1.
// dlfo_eh_frame is the PT_GNU_EH_FRAME segment -- i.e. .eh_frame_hdr, NOT .eh_frame, despite the
// name. Verified against the disassembly: libgcc loads that field and immediately checks byte 0
// == 1, which is the .eh_frame_hdr version field.
struct nyx_dl_find_object {
    unsigned long long dlfo_flags;
    void *dlfo_map_start;   // beginning of the mapping containing the address
    void *dlfo_map_end;     // end of it
    void *dlfo_link_map;    // no dynamic loader here -- NULL, and libgcc never derefs it
    void *dlfo_eh_frame;    // -> .eh_frame_hdr (offset 32; matches libgcc's -0x70(%rbp) load)
    unsigned long long __reserved[7];
};

// ld defines this at the ELF header whenever the header is inside a PT_LOAD, which it is here
// (first LOAD covers file offset 0). Referenced strongly ON PURPOSE: if the link ever stops
// providing it, the build breaks loudly instead of silently reverting to "no exceptions".
extern const char __ehdr_start[];

int _dl_find_object(void *address, void *result) {
    struct nyx_dl_find_object *r = (struct nyx_dl_find_object *)result;
    const Elf64_Ehdr *eh = (const Elf64_Ehdr *)__ehdr_start;
    const Elf64_Phdr *ph = (const Elf64_Phdr *)(__ehdr_start + eh->e_phoff);

    // Load bias. PT_PHDR states it directly; a static non-PIE link (what Nyx uses -- and what the
    // kernel's loader expects, since it honours p_vaddr literally) emits no PT_PHDR and has a
    // bias of 0. Computing it anyway keeps this correct if these binaries ever go PIE.
    uintptr_t bias = 0;
    for (unsigned i = 0; i < eh->e_phnum; i++)
        if (ph[i].p_type == PT_PHDR) { bias = (uintptr_t)ph - (uintptr_t)ph[i].p_vaddr; break; }

    uintptr_t lo = (uintptr_t)-1, hi = 0, ehframe = 0;
    for (unsigned i = 0; i < eh->e_phnum; i++) {
        uintptr_t start = bias + (uintptr_t)ph[i].p_vaddr;
        if (ph[i].p_type == PT_LOAD) {
            if (start < lo) lo = start;
            if (start + ph[i].p_memsz > hi) hi = start + ph[i].p_memsz;
        } else if (ph[i].p_type == PT_GNU_EH_FRAME) {
            ehframe = start;
        }
    }

    // No PT_GNU_EH_FRAME means the unwind tables are unreachable and saying so is the honest
    // answer -- this is exactly the state the missing -Wl,--eh-frame-hdr left the binary in.
    if (!ehframe || lo > hi) return -1;

    uintptr_t a = (uintptr_t)address;
    if (a < lo || a >= hi) return -1;

    r->dlfo_flags = 0;
    r->dlfo_map_start = (void *)lo;
    r->dlfo_map_end = (void *)hi;
    r->dlfo_link_map = 0;
    r->dlfo_eh_frame = (void *)ehframe;
    for (unsigned i = 0; i < 7; i++) r->__reserved[i] = 0;
    return 0;
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
