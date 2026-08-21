// musl smoke test — measure what libc++ and Ladybird will actually stand on.
//
// hello.c proved musl STARTS. This measures whether it WORKS: allocation at scale, stdio beyond
// a single printf, string/number formatting, time, and threads. The point is to convert "unknown
// gaps" into a counted work list BEFORE committing to a long libc++ build, rather than
// discovering them as a link error or a hang halfway through one. Same method as tests/posix.
//
// Conventions carried over from the POSIX harness, each of which was learned the hard way:
//   - stdout is set UNBUFFERED. If a probe kills the process, buffered output is lost and the
//     log ends before the probe that died is even named -- which is indistinguishable from the
//     probe never having run.
//   - Every probe PRINTS ITS NAME BEFORE RUNNING, so a hang is attributable to a specific line.
//   - A probe passes on a verified EFFECT, not on a return code. Writing a pattern and reading
//     it back is the point; malloc returning non-NULL proves nothing about the memory.
//   - The probe most likely to take the process down runs LAST, so a failure there cannot cost
//     us the results of everything before it.

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <time.h>
#include <pthread.h>

static int n_pass = 0, n_fail = 0;

static void announce(const char *name) { printf("smoke: %-20s ... ", name); fflush(stdout); }
static void ok(void)                   { n_pass++; printf("PASS\n"); fflush(stdout); }
static void no(const char *why)        { n_fail++; printf("FAIL (%s)\n", why); fflush(stdout); }
static void note(const char *what)     { printf("  note: %s\n", what); fflush(stdout); }

// Fill with a position-dependent pattern, so a partial or overlapping copy is detectable.
// memset with a constant would pass even if the allocator handed back overlapping blocks.
static void fill(unsigned char *p, size_t n, unsigned seed) {
    for (size_t i = 0; i < n; i++) p[i] = (unsigned char)((i * 31u + seed) & 0xFF);
}
static int verify(const unsigned char *p, size_t n, unsigned seed) {
    for (size_t i = 0; i < n; i++)
        if (p[i] != (unsigned char)((i * 31u + seed) & 0xFF)) return 0;
    return 1;
}

// ── allocation ────────────────────────────────────────────────────────────────────────────────

static void probe_malloc_small(void) {
    announce("malloc small x64");
    unsigned char *blocks[64];
    for (int i = 0; i < 64; i++) {
        blocks[i] = malloc(128);
        if (!blocks[i]) { no("malloc returned NULL"); return; }
        fill(blocks[i], 128, (unsigned)i);
    }
    // Verify AFTER every block exists: this is what catches an allocator handing out
    // overlapping regions, which a fill-then-check-immediately loop would miss entirely.
    for (int i = 0; i < 64; i++)
        if (!verify(blocks[i], 128, (unsigned)i)) { no("blocks overlap or were corrupted"); return; }
    for (int i = 0; i < 64; i++) free(blocks[i]);
    ok();
}

static void probe_malloc_large(void) {
    announce("malloc 4 MiB");
    // Well past any small-bin threshold, so this exercises the mmap path. Relevant because
    // Nyx's brk(12) is a stub returning 0 -- musl is expected to fall back to mmap, and this
    // is the row that says whether that fallback actually holds.
    size_t n = 4u * 1024 * 1024;
    unsigned char *p = malloc(n);
    if (!p) { no("malloc(4MiB) returned NULL"); return; }
    // Touch the boundaries and the middle rather than all 4 MiB: enough to fault in the first
    // and last page, which is where a short mapping shows up.
    fill(p, 4096, 7);
    fill(p + n - 4096, 4096, 9);
    if (!verify(p, 4096, 7) || !verify(p + n - 4096, 4096, 9)) { no("large block not readable back"); free(p); return; }
    free(p);
    ok();
}

static void probe_realloc(void) {
    announce("realloc grow");
    unsigned char *p = malloc(1024);
    if (!p) { no("malloc"); return; }
    fill(p, 1024, 3);
    unsigned char *q = realloc(p, 64 * 1024);
    if (!q) { no("realloc returned NULL"); free(p); return; }
    if (!verify(q, 1024, 3)) { no("contents not preserved across grow"); free(q); return; }
    free(q);
    ok();
}

static void probe_calloc(void) {
    announce("calloc zeroing");
    unsigned char *p = calloc(4096, 1);
    if (!p) { no("calloc returned NULL"); return; }
    for (int i = 0; i < 4096; i++)
        if (p[i] != 0) { no("memory not zeroed"); free(p); return; }
    free(p);
    ok();
}

// ── formatting and algorithms ─────────────────────────────────────────────────────────────────

static void probe_snprintf(void) {
    announce("snprintf/strtod");
    char buf[64];
    int len = snprintf(buf, sizeof buf, "%d %s %.3f %#x", -42, "abc", 3.5, 255);
    if (len <= 0) { no("snprintf returned <= 0"); return; }
    if (strcmp(buf, "-42 abc 3.500 0xff") != 0) { no(buf); return; }
    // Float formatting is a real dependency, not a curiosity: it drags in musl's whole
    // long-double path, which is the sort of thing that silently emits garbage rather than
    // failing loudly.
    if (strtod("2.5", NULL) != 2.5) { no("strtod"); return; }
    ok();
}

static int cmp_int(const void *a, const void *b) { return *(const int *)a - *(const int *)b; }

static void probe_qsort(void) {
    announce("qsort");
    int v[8] = { 5, 3, 9, 1, 7, 2, 8, 4 };
    qsort(v, 8, sizeof(int), cmp_int);
    for (int i = 1; i < 8; i++)
        if (v[i - 1] > v[i]) { no("not sorted"); return; }
    ok();
}

// ── stdio ─────────────────────────────────────────────────────────────────────────────────────

static void probe_stdio_file(void) {
    announce("stdio file rw");
    const char *path = "/mnt/nvme/apps/HelloC.nyx/smoke.tmp";
    const char *msg  = "musl stdio round trip\n";

    FILE *f = fopen(path, "w");
    if (!f) { no("fopen for write"); return; }
    if (fwrite(msg, 1, strlen(msg), f) != strlen(msg)) { no("fwrite short"); fclose(f); return; }
    if (fclose(f) != 0) { no("fclose"); return; }

    f = fopen(path, "r");
    if (!f) { no("fopen for read"); return; }
    char back[64] = { 0 };
    size_t got = fread(back, 1, sizeof back - 1, f);
    if (got != strlen(msg) || strcmp(back, msg) != 0) { no("contents differ"); fclose(f); return; }

    // fseek/ftell separately: buffered positioning is its own machinery, and it is what
    // anything parsing a file (every asset loader in a browser) actually leans on.
    if (fseek(f, 6, SEEK_SET) != 0) { no("fseek"); fclose(f); return; }
    if (ftell(f) != 6)              { no("ftell"); fclose(f); return; }
    int c = fgetc(f);
    if (c != msg[6])                { no("fgetc after seek"); fclose(f); return; }
    fclose(f);
    remove(path);
    ok();
}

// ── time ──────────────────────────────────────────────────────────────────────────────────────

static void probe_clock(void) {
    announce("clock_gettime");
    struct timespec a, b;
    if (clock_gettime(CLOCK_MONOTONIC, &a) != 0) { no("clock_gettime failed"); return; }
    for (volatile int i = 0; i < 2000000; i++) { }
    if (clock_gettime(CLOCK_MONOTONIC, &b) != 0) { no("second call failed"); return; }
    // Monotonic must not go backwards. A clock that returns a constant reads as "working" to
    // any single-call test, and then every timeout in an event loop is either 0 or infinite.
    long long da = (long long)a.tv_sec * 1000000000LL + a.tv_nsec;
    long long db = (long long)b.tv_sec * 1000000000LL + b.tv_nsec;
    if (db <= da) { no("clock did not advance"); return; }
    ok();
}

// ── environment ───────────────────────────────────────────────────────────────────────────────

static void probe_environ(void) {
    announce("environ/getenv");
    extern char **environ;
    if (!environ) { no("environ is NULL"); return; }
    int count = 0;
    for (char **e = environ; *e; e++) count++;
    // Not a failure if empty -- Nyx's build_initial_stack may pass no envp. What matters is
    // that the array is well-formed and terminated, because libc++ walks it.
    if (count == 0) note("environ is empty (well-formed, but nothing is passed)");
    ok();
}

// ── threads (LAST: most likely to take the process down) ──────────────────────────────────────

static void *thread_body(void *arg) { *(int *)arg = 0x5A; return arg; }

static void probe_pthread(void) {
    announce("pthread_create");
    // Runs last on purpose. Nyx implements threads as its OWN syscall 58 (sys_spawn_thread) and
    // has NO clone(2) arm, which is what musl's pthread_create issues -- so this is expected to
    // fail. It is here to report HOW it fails: a clean errno means the shim is the only work,
    // whereas a hang or a fault means clone lands somewhere that half-works, which is a much
    // more dangerous starting point.
    pthread_t t;
    int marker = 0;
    int rc = pthread_create(&t, NULL, thread_body, &marker);
    if (rc != 0) {
        printf("EXPECTED-FAIL (rc=%d %s)\n", rc, strerror(rc));
        fflush(stdout);
        note("clone(2) is unimplemented; threads are Nyx syscall 58. This is the next kernel task.");
        return; // deliberately counted as neither pass nor fail
    }
    if (pthread_join(t, NULL) != 0) { no("pthread_join"); return; }
    if (marker != 0x5A)             { no("thread body did not run"); return; }
    ok();
}

int main(void) {
    // Unbuffered BEFORE anything else: if a probe kills the process, buffered output dies with
    // it and the log stops before naming the probe that died.
    setvbuf(stdout, NULL, _IONBF, 0);

    printf("=== musl smoke test on Nyx ===\n");

    probe_malloc_small();
    probe_malloc_large();
    probe_realloc();
    probe_calloc();
    probe_snprintf();
    probe_qsort();
    probe_stdio_file();
    probe_clock();
    probe_environ();
    probe_pthread();

    printf("=== SUMMARY pass=%d fail=%d ===\n", n_pass, n_fail);
    return n_fail == 0 ? 0 : 1;
}
