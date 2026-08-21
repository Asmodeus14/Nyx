// C++ ABI gate: does the *language* work on Nyx, before libc++ is ever built?
//
// libc++ is a large dependency needing cmake. But the interesting question is not the library --
// it is the ABI runtime underneath it: exceptions, RTTI, guarded static initialisation, operator
// new, and global constructors. gcc already ships that as libsupc++.a + libgcc_eh.a, so this
// links those against musl and answers the question for the cost of one compile.
//
// Exceptions are the real test. Throwing needs the unwinder to find .eh_frame, which for a static
// binary it does via dl_iterate_phdr, which musl answers from AT_PHDR/AT_PHNUM -- auxv entries
// Nyx's build_initial_stack already provides. So a working `throw` here also confirms that the
// ELF loader's auxv is correct, which nothing has ever really depended on until now.
//
// DELIBERATELY NO #include. gcc's C++ headers live outside the musl include path and drag in
// glibc-specific os_defines/cpu_defines; fighting that would test the header layout rather than
// the ABI. Everything needed is declared here, so what links is exactly what is under test.

extern "C" int printf(const char *, ...);
extern "C" void *malloc(unsigned long);

// Supplied by libsupc++.
void *operator new(unsigned long);
void *operator new[](unsigned long);
void operator delete(void *) noexcept;
void operator delete[](void *) noexcept;
void operator delete(void *, unsigned long) noexcept;
void operator delete[](void *, unsigned long) noexcept;

static int n_pass = 0, n_fail = 0;
static void ok(const char *name)                 { n_pass++; printf("cpp: %-22s PASS\n", name); }
static void no(const char *name, const char *why) { n_fail++; printf("cpp: %-22s FAIL (%s)\n", name, why); }

// ── global constructors ───────────────────────────────────────────────────────────────────────
// Runs before main only if musl's __libc_start_main walks .init_array. Untested until now: every
// Nyx binary so far has been Rust or C, neither of which needs it.
static int g_ctor_ran = 0;
struct GlobalCtor { GlobalCtor() { g_ctor_ran = 0xC7; } };
static GlobalCtor g_ctor_instance;

// ── virtual dispatch + RTTI ───────────────────────────────────────────────────────────────────
struct Base         { virtual ~Base() {} virtual int id() const { return 1; } };
struct Derived : Base { int id() const override { return 2; } int extra() const { return 42; } };

// ── guarded static init (uses __cxa_guard_acquire, and futex under threads) ───────────────────
struct Counted { int v; Counted() : v(0x5A) {} };
static Counted &counted() { static Counted c; return c; }

// ── exceptions ────────────────────────────────────────────────────────────────────────────────
struct MyError { int code; };

static int thrower(int x) {
    if (x > 0) throw MyError{ x * 2 };
    return -1;
}

int main() {
    printf("=== C++ ABI test on Nyx (musl + libsupc++) ===\n");

    if (g_ctor_ran == 0xC7) ok("global ctor");
    else                    no("global ctor", ".init_array did not run");

    // operator new / delete, and that the object is actually usable rather than merely non-null.
    {
        Derived *d = new Derived();
        if (d && d->id() == 2 && d->extra() == 42) ok("new/delete + virtual");
        else                                       no("new/delete + virtual", "bad object");
        delete d;
    }

    // Virtual dispatch through a base pointer -- the vtable must have survived the ELF load.
    {
        Derived d;
        Base *b = &d;
        if (b->id() == 2) ok("virtual dispatch");
        else              no("virtual dispatch", "called the base implementation");
    }

    // RTTI. dynamic_cast has to compare type_info across the hierarchy, which needs the
    // typeinfo symbols the loader placed in .rodata to be reachable.
    {
        Derived d;
        Base *b = &d;
        Derived *back = dynamic_cast<Derived *>(b);
        Base base_only;
        Derived *should_be_null = dynamic_cast<Derived *>(&base_only);
        if (back == &d && should_be_null == nullptr) ok("dynamic_cast (RTTI)");
        else                                         no("dynamic_cast (RTTI)", "wrong result");
    }

    // Guarded static init: __cxa_guard_acquire/release. Two calls must yield the same object
    // constructed exactly once.
    {
        Counted &a = counted();
        Counted &b = counted();
        if (&a == &b && a.v == 0x5A) ok("static init guard");
        else                         no("static init guard", "constructed twice or wrong value");
    }

    // The main event: throw across a function boundary and unwind back. This exercises
    // __cxa_throw, the personality routine, and .eh_frame lookup via dl_iterate_phdr/auxv.
    {
        int caught = 0;
        try { thrower(21); }
        catch (const MyError &e) { caught = e.code; }
        catch (...)              { caught = -1; }
        if (caught == 42) ok("throw/catch typed");
        else              no("throw/catch typed", caught == -1 ? "caught wrong type" : "not caught");
    }

    // Unwinding must run destructors on the way out, which is the part that makes exceptions
    // usable rather than merely survivable. A leak here is silent in every other test.
    {
        static int destroyed = 0;
        struct Guard { ~Guard() { destroyed++; } };
        try { Guard g; thrower(1); }
        catch (const MyError &) { }
        if (destroyed == 1) ok("unwind runs dtors");
        else                no("unwind runs dtors", "destructor skipped during unwind");
    }

    // Rethrow through a second frame: catch, rethrow, catch again. Nested unwinding uses a
    // different libsupc++ path (__cxa_rethrow) than a plain throw.
    {
        int outer = 0;
        try {
            try { thrower(5); }
            catch (const MyError &) { throw; }
        } catch (const MyError &e) { outer = e.code; }
        if (outer == 10) ok("rethrow");
        else             no("rethrow", "lost during rethrow");
    }

    printf("=== SUMMARY pass=%d fail=%d ===\n", n_pass, n_fail);
    return n_fail == 0 ? 0 : 1;
}
