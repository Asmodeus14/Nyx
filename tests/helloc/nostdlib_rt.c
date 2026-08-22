// The bits that crtbegin.o would normally supply, for a -nostdlib link.
//
// This is NOT a Nyx workaround and NOT a musl gap -- it is a consequence of -nostdlib, which
// suppresses gcc's crtbegin.o/crtend.o along with everything else. musl's crt1.o deliberately
// provides only _start and the __libc_start_main call; the C++ support objects come from the
// compiler's runtime, not the libc's.
//
// Used by the libc++ link only (see build.sh::build_libcxx). The libsupc++ link does not need it,
// because nothing in that much smaller slice of the ABI registers a static destructor.

// __cxa_atexit(destructor, arg, dso_handle) is how a C++ static destructor gets registered, and
// the third argument identifies the shared object owning it so that dlclose() can run just that
// object's destructors. A static executable is never unloaded and has exactly one "object", so
// the value is only ever compared for equality and never dereferenced.
//
// gcc's crtstuff.c uses NULL for an executable (crtbegin.o) and &__dso_handle for a shared library
// (crtbeginS.o). This matches the executable case, which is the only one Nyx has -- there is no
// dynamic loader.
//
// ★ `hidden` visibility is REQUIRED, not stylistic: libc++.a's references are to a hidden symbol,
// and ld refuses to bind those to a default-visibility definition -- the error is
// "hidden symbol `__dso_handle' isn't defined", which reads like the definition is missing
// entirely rather than merely mismatched.
__attribute__((visibility("hidden"))) void *__dso_handle = 0;
