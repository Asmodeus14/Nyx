// Phase 6, gate 3: can Nyx host an EVENT LOOP?
//
// Gate 2 proved libc++ works. This asks the question gate 3 actually turns on, which the earlier
// probes could not: do poll, signals, timers, threads and file I/O work *together, under load*?
// Every one of those already passes in isolation. An event loop is the first thing that runs them
// concurrently and depends on the interactions -- which is where the remaining kernel gaps live.
//
// This is deliberately NOT a Ladybird port. Ladybird is enormous, and starting by dragging it in
// would mean debugging a kernel through a 500k-line build system. This reproduces the mechanics
// LibCore::EventLoop actually uses, so a failure here names a kernel gap directly:
//
//   - the SELF-PIPE TRICK: a signal handler writes one byte to a pipe and the loop polls that
//     pipe. This is how LibCore turns an async signal into a pollable event, and it needs a
//     handler to run, write(2) to be usable from inside one, and poll to see the result.
//   - TIMERS as a poll timeout: the deadline nearest to now becomes poll's timeout argument.
//     Needs a monotonic clock that agrees with poll's own sense of time -- if they disagree the
//     loop either spins or oversleeps.
//   - CROSS-THREAD WAKEUP: a worker thread writes to a wakeup pipe (LibCore::EventLoop::wake()),
//     which needs a pipe write from one thread to wake a poll on another.
//   - EINTR: the loop must RETRY rather than treat it as failure, so this depends on EINTR both
//     existing and being correctly attributed.
//
// ★ Discipline carried from cxxtest.cpp / smoke.c:
//   - announce BEFORE running, so a hang names the probe that hung
//   - flush every line -- a buffered log is lost exactly when something goes wrong
//   - verify an EFFECT, not a return code
//   - ★ EVERY blocking call has a finite timeout and every loop has an iteration cap. Nyx is
//     tested on bare metal, so a probe that can hang costs a power cycle. There is no infinite
//     poll anywhere in this file, on purpose.

#include <atomic>
#include <cerrno>
#include <chrono>
#include <csignal>
#include <cstdio>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <unistd.h>

static int n_pass = 0, n_fail = 0;

static void announce(const char *name) {
    std::printf("evt: %-28s ... ", name);
    std::fflush(stdout);
}
static void ok()                { n_pass++; std::printf("PASS\n"); std::fflush(stdout); }
static void no(const char *why) { n_fail++; std::printf("FAIL (%s)\n", why); std::fflush(stdout); }

// ---------------------------------------------------------------------------------------------
// The self-pipe. A signal handler may not allocate, lock, or call printf; writing one byte to a
// pipe is the async-signal-safe primitive every event loop is built on.
// ---------------------------------------------------------------------------------------------
static int g_sigpipe[2] = {-1, -1};
static volatile std::sig_atomic_t g_sig_count = 0;

static void self_pipe_handler(int sig) {
    g_sig_count++;
    unsigned char b = static_cast<unsigned char>(sig);
    // Return value deliberately ignored and cast away: there is nothing safe to do about a failed
    // write from inside a handler, and -Wunused-result would otherwise fire.
    ssize_t n = ::write(g_sigpipe[1], &b, 1);
    (void)n;
}

static std::chrono::steady_clock::time_point now() { return std::chrono::steady_clock::now(); }

static long ms_since(std::chrono::steady_clock::time_point t0) {
    return std::chrono::duration_cast<std::chrono::milliseconds>(now() - t0).count();
}

// poll(2) with the one behaviour every event loop needs: EINTR is not an error, it means "go
// round again with a recomputed timeout". Returning it to the caller is the classic bug that makes
// a loop die the first time a signal arrives.
static int poll_retrying(struct pollfd *fds, nfds_t n, int timeout_ms, int *eintrs) {
    auto deadline = now() + std::chrono::milliseconds(timeout_ms < 0 ? 0 : timeout_ms);
    for (int guard = 0; guard < 64; guard++) { // capped: never spin forever on a broken errno
        int r = ::poll(fds, n, timeout_ms);
        if (r >= 0) return r;
        if (errno != EINTR) return -1;
        if (eintrs) (*eintrs)++;
        long left = std::chrono::duration_cast<std::chrono::milliseconds>(deadline - now()).count();
        if (left <= 0) return 0;
        timeout_ms = static_cast<int>(left);
    }
    return -1;
}

int main() {
    std::setvbuf(stdout, nullptr, _IONBF, 0);
    std::printf("=== Nyx event loop (Phase 6, gate 3) ===\n");
    std::fflush(stdout);

    // --- 1. pipes + poll readiness ------------------------------------------------------------
    announce("pipe + poll readiness");
    int p[2];
    if (::pipe(p) != 0) {
        no("pipe() failed");
    } else {
        struct pollfd pf = {p[0], POLLIN, 0};
        int before = ::poll(&pf, 1, 0);            // nothing written yet
        ssize_t w = ::write(p[1], "x", 1);
        pf.revents = 0;
        int after = ::poll(&pf, 1, 50);
        if (before == 0 && w == 1 && after == 1 && (pf.revents & POLLIN))
            ok();
        else
            no("readiness did not change after a write");
    }

    // --- 2. the self-pipe trick ---------------------------------------------------------------
    // The heart of it: an async signal turned into a pollable event.
    announce("self-pipe signal wakeup");
    if (::pipe(g_sigpipe) != 0) {
        no("pipe() failed");
    } else {
        struct sigaction sa {};
        sa.sa_handler = self_pipe_handler;
        sigemptyset(&sa.sa_mask);
        sa.sa_flags = 0;
        if (::sigaction(SIGUSR1, &sa, nullptr) != 0) {
            no("sigaction refused the handler");
        } else {
            ::raise(SIGUSR1);
            struct pollfd pf = {g_sigpipe[0], POLLIN, 0};
            int r = ::poll(&pf, 1, 200);
            unsigned char got = 0;
            ssize_t n = (r == 1) ? ::read(g_sigpipe[0], &got, 1) : 0;
            if (r == 1 && n == 1 && got == SIGUSR1 && g_sig_count == 1)
                ok();
            else
                no("signal did not arrive through the pipe");
        }
    }

    // --- 3. timers as a poll timeout ----------------------------------------------------------
    // poll's timeout and steady_clock must agree, or a loop either spins or oversleeps.
    announce("timer via poll timeout");
    {
        struct pollfd pf = {p[0], POLLIN, 0};
        // p[0] still holds the byte from probe 1; drain it so this really blocks.
        unsigned char drain;
        ssize_t dn = ::read(p[0], &drain, 1);
        (void)dn;
        auto t0 = now();
        int r = ::poll(&pf, 1, 150);
        long took = ms_since(t0);
        // Generous window: this measures that the clock and the timeout are not wildly
        // inconsistent, not that scheduling is precise.
        if (r == 0 && took >= 100 && took < 1000)
            ok();
        else
            no("poll timeout and steady_clock disagree");
    }

    // --- 4. ordered timers --------------------------------------------------------------------
    announce("timer ordering");
    {
        struct Timer { long due_ms; int id; };
        std::vector<Timer> timers = {{120, 3}, {40, 1}, {80, 2}};
        std::vector<int> fired;
        auto t0 = now();
        for (int guard = 0; guard < 16 && fired.size() < timers.size(); guard++) {
            long soonest = -1;
            for (auto &t : timers) {
                bool already = false;
                for (int f : fired) already |= (f == t.id);
                if (already) continue;
                long left = t.due_ms - ms_since(t0);
                if (left < 0) left = 0;
                if (soonest < 0 || left < soonest) soonest = left;
            }
            if (soonest < 0) break;
            struct pollfd pf = {p[0], POLLIN, 0};
            ::poll(&pf, 1, static_cast<int>(soonest)); // no fd will fire; this IS the timer
            long el = ms_since(t0);
            for (auto &t : timers) {
                bool already = false;
                for (int f : fired) already |= (f == t.id);
                if (!already && el + 5 >= t.due_ms) fired.push_back(t.id);
            }
        }
        if (fired.size() == 3 && fired[0] == 1 && fired[1] == 2 && fired[2] == 3)
            ok();
        else
            no("timers fired out of order or not at all");
    }

    // --- 5. many fds at once ------------------------------------------------------------------
    announce("multi-fd readiness");
    {
        const int N = 8;
        int fds[N][2];
        bool made = true;
        for (int i = 0; i < N; i++) if (::pipe(fds[i]) != 0) { made = false; break; }
        if (!made) {
            no("could not create 8 pipes");
        } else {
            // Write to the odd ones only: the loop must report exactly those.
            for (int i = 1; i < N; i += 2) { ssize_t x = ::write(fds[i][1], "y", 1); (void)x; }
            struct pollfd pf[N];
            for (int i = 0; i < N; i++) pf[i] = {fds[i][0], POLLIN, 0};
            int r = poll_retrying(pf, N, 200, nullptr);
            int ready = 0; bool correct = true;
            for (int i = 0; i < N; i++) {
                bool hot = (pf[i].revents & POLLIN) != 0;
                if (hot) ready++;
                if (hot != (i % 2 == 1)) correct = false;
            }
            for (int i = 0; i < N; i++) { ::close(fds[i][0]); ::close(fds[i][1]); }
            if (r == N / 2 && ready == N / 2 && correct)
                ok();
            else
                no("wrong set of descriptors reported ready");
        }
    }

    // --- 6. cross-thread wakeup ---------------------------------------------------------------
    // LibCore::EventLoop::wake() -- another thread makes a parked poll return.
    announce("cross-thread wakeup");
    {
        int wake[2];
        if (::pipe(wake) != 0) {
            no("pipe() failed");
        } else {
            std::atomic<bool> started{false};
            std::thread worker([&] {
                started = true;
                std::this_thread::sleep_for(std::chrono::milliseconds(80));
                ssize_t x = ::write(wake[1], "w", 1);
                (void)x;
            });
            struct pollfd pf = {wake[0], POLLIN, 0};
            auto t0 = now();
            int r = poll_retrying(&pf, 1, 1000, nullptr);
            long took = ms_since(t0);
            worker.join();
            ::close(wake[0]); ::close(wake[1]);
            // Must be woken BY the write, not by the timeout.
            if (r == 1 && (pf.revents & POLLIN) && took < 900)
                ok();
            else
                no("a poll on one thread was not woken by another");
        }
    }

    // --- 7. EINTR is retried, not fatal -------------------------------------------------------
    // The kernel change this gate depended on, seen from the consumer's side.
    announce("EINTR retried by the loop");
    {
        int quiet[2];
        if (::pipe(quiet) != 0) {
            no("pipe() failed");
        } else {
            g_sig_count = 0;
            // ★ pthread_kill, NOT raise(). raise() signals the CALLING thread, so the signaller
            // would deliver to itself and the polling thread would never see EINTR -- the probe
            // then passes without exercising the thing it exists to test. (It did exactly that on
            // the first host run: "eintrs observed: 0".) Targeting the parked thread by handle is
            // what forces the interrupt, and it routes through tgkill.
            pthread_t parked = ::pthread_self();
            std::thread signaller([&] {
                std::this_thread::sleep_for(std::chrono::milliseconds(60));
                ::pthread_kill(parked, SIGUSR1);
            });
            struct pollfd pf = {quiet[0], POLLIN, 0};
            int eintrs = 0;
            auto t0 = now();
            int r = poll_retrying(&pf, 1, 300, &eintrs);
            long took = ms_since(t0);
            signaller.join();
            ::close(quiet[0]); ::close(quiet[1]);
            // The full contract: the poll was interrupted (eintrs >= 1), the loop RETRIED rather
            // than failing, it still served out its remaining timeout, and it never invented
            // readiness on a pipe nobody wrote to.
            if (r < 0)
                no("poll failed instead of retrying");
            else if (pf.revents & POLLIN)
                no("poll returned readiness on a silent pipe");
            else if (eintrs == 0)
                no("signal never interrupted the poll -- no EINTR");
            else if (took < 250)
                no("returned early: the retry did not serve the rest of the timeout");
            else
                ok();
            std::printf("     (eintrs observed: %d, handler runs: %d)\n",
                        eintrs, static_cast<int>(g_sig_count));
            std::fflush(stdout);
        }
    }

    // --- 8. file I/O interleaved with the loop ------------------------------------------------
    // The combination gate 3 is really about: a loop that also touches the filesystem.
    announce("file I/O inside the loop");
    {
        const char *path = "/mnt/nvme/apps/HelloC.nyx/evtloop.tmp";
        bool nvme = (::access("/mnt/nvme", F_OK) == 0);
        if (!nvme) path = "/tmp/evtloop.tmp";
        bool good = true;
        for (int i = 0; i < 4 && good; i++) {
            int fd = ::open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
            if (fd < 0) { good = false; break; }
            char buf[32];
            int len = std::snprintf(buf, sizeof buf, "round-%d", i);
            good = (::write(fd, buf, len) == len);
            ::close(fd);
            // A poll between each round, so file I/O and the loop genuinely interleave.
            struct pollfd pf = {p[0], POLLIN, 0};
            ::poll(&pf, 1, 10);
            int rfd = ::open(path, O_RDONLY);
            if (rfd < 0) { good = false; break; }
            char back[32] = {0};
            ssize_t n = ::read(rfd, back, sizeof back - 1);
            ::close(rfd);
            good = good && (n == len) && (std::strcmp(back, buf) == 0);
        }
        ::unlink(path);
        if (good) ok(); else no("write/poll/read round trip broke down");
    }

    // --- 9. sustained loop --------------------------------------------------------------------
    // Everything at once, repeatedly. A single pass can pass by luck; a fd or memory leak shows up
    // here as a failure partway through.
    announce("sustained loop (200 iterations)");
    {
        int a[2], b[2];
        if (::pipe(a) != 0 || ::pipe(b) != 0) {
            no("pipe() failed");
        } else {
            bool good = true;
            int served = 0;
            for (int i = 0; i < 200 && good; i++) {
                int which = (i % 2) ? b[1] : a[1];
                ssize_t x = ::write(which, "z", 1);
                if (x != 1) { good = false; break; }
                struct pollfd pf[2] = {{a[0], POLLIN, 0}, {b[0], POLLIN, 0}};
                int r = poll_retrying(pf, 2, 200, nullptr);
                if (r != 1) { good = false; break; }
                for (int k = 0; k < 2; k++) {
                    if (pf[k].revents & POLLIN) {
                        unsigned char c;
                        if (::read(pf[k].fd, &c, 1) == 1) served++;
                    }
                }
            }
            ::close(a[0]); ::close(a[1]); ::close(b[0]); ::close(b[1]);
            if (good && served == 200) ok(); else no("loop degraded before 200 iterations");
        }
    }

    ::close(p[0]); ::close(p[1]);
    ::close(g_sigpipe[0]); ::close(g_sigpipe[1]);

    std::printf("=== SUMMARY pass=%d fail=%d ===\n", n_pass, n_fail);
    std::fflush(stdout);
    return n_fail == 0 ? 0 : 1;
}
