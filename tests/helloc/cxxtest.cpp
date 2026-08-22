// Phase 6, gate 2: does the C++ STANDARD LIBRARY work on Nyx?
//
// cpptest.cpp answered the ABI question (exceptions, RTTI, guards, vtables) using gcc's
// libsupc++. This is the layer above: real libc++ built against musl, exercising the parts of the
// library that actually touch the kernel -- iostream, threads, chrono, filesystem -- rather than
// the parts that are pure templates and would compile anywhere.
//
// Unlike cpptest.cpp this one #includes freely: libc++'s headers are built for musl, so there is
// no glibc entanglement to route around.
//
// ★ Discipline carried from smoke.c, deliberately, because forgetting it once already cost a boot:
//   - announce the probe name BEFORE running it, so a hang is attributable to a specific probe
//   - flush after every line -- abort() and _Exit() do NOT flush, so a buffered log is lost
//     precisely when something goes wrong, which reads as "died immediately" instead of "died at
//     probe 7"
//   - verify an EFFECT, not just that a call returned
//   - riskiest probe last

#include <algorithm>
#include <chrono>
#include <cstdio>
#include <exception>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <map>
#include <memory>
#include <mutex>
#include <regex>
#include <sstream>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

static int n_pass = 0, n_fail = 0;

static void announce(const char *name) {
    std::printf("cxx: %-24s ... ", name);
    std::fflush(stdout);
}
static void ok()                     { n_pass++; std::printf("PASS\n"); std::fflush(stdout); }
static void no(const char *why)      { n_fail++; std::printf("FAIL (%s)\n", why); std::fflush(stdout); }

// Nyx and the WSL host are both valid places to run this -- the binary is static musl and the
// syscalls are Linux-shaped, so the host is the zero-power-cycle way to test it. Pick a scratch
// directory that exists in whichever one we are on.
static std::filesystem::path scratch_root() {
    std::error_code ec;
    if (std::filesystem::exists("/mnt/nvme", ec)) return "/mnt/nvme/apps/HelloC.nyx";
    return "/tmp";
}

int main() {
    // iostream and printf are both used below; without this they can interleave out of order.
    std::ios::sync_with_stdio(true);

    std::printf("=== libc++ on Nyx (libc++ + libc++abi + libunwind on musl) ===\n");
    std::fflush(stdout);

    announce("string");
    {
        std::string s = "Nyx";
        s += " runs " ;
        s.append("libc++");
        if (s == "Nyx runs libc++" && s.find("runs") == 4 && s.substr(4, 4) == "runs") ok();
        else no("string ops wrong");
    }

    announce("vector + sort");
    {
        std::vector<int> v{5, 3, 9, 1, 7};
        std::sort(v.begin(), v.end());
        auto it = std::lower_bound(v.begin(), v.end(), 7);
        if (v.front() == 1 && v.back() == 9 && it != v.end() && *it == 7) ok();
        else no("sort/lower_bound wrong");
    }

    announce("map + iteration order");
    {
        std::map<std::string, int> m{{"pear", 2}, {"apple", 1}, {"fig", 3}};
        std::string keys;
        for (const auto &kv : m) keys += kv.first + ",";
        // std::map iterates in sorted key order; anything else means the tree is broken.
        if (keys == "apple,fig,pear," && m.at("fig") == 3 && m.count("plum") == 0) ok();
        else no("map order/lookup wrong");
    }

    announce("smart pointers");
    {
        auto u = std::make_unique<std::string>("unique");
        auto s1 = std::make_shared<std::vector<int>>(std::vector<int>{1, 2, 3});
        auto s2 = s1;
        // use_count proves the control block is shared rather than each copy owning its own.
        if (*u == "unique" && s1->size() == 3 && s2.use_count() == 2) ok();
        else no("ownership wrong");
    }

    announce("stringstream");
    {
        std::ostringstream os;
        os << "n=" << 42 << " f=" << 2.5 << " s=" << std::string("x");
        std::istringstream is("7 hello");
        int n = 0; std::string w;
        is >> n >> w;
        if (os.str() == "n=42 f=2.5 s=x" && n == 7 && w == "hello") ok();
        else no("stream formatting/parsing wrong");
    }

    announce("iostream (cout)");
    {
        // std::cout is the interesting one: it is constructed by a global initialiser in libc++
        // (ios_base::Init), so this also re-proves .init_array ordering with a real dependency.
        std::cout << "" << std::flush;
        if (std::cout.good()) ok();
        else no("cout not usable");
    }

    announce("exceptions (std::)");
    {
        int caught = 0;
        std::string what;
        try { throw std::runtime_error("boom"); }
        catch (const std::exception &e) { caught = 1; what = e.what(); }
        // out_of_range is thrown from INSIDE libc++.a rather than from this translation unit --
        // a different unwind path, across an archive boundary.
        int caught2 = 0;
        try { std::vector<int> v(3); (void)v.at(99); }
        catch (const std::out_of_range &) { caught2 = 1; }
        if (caught && what == "boom" && caught2) ok();
        else no(caught ? "library-thrown exception not caught" : "not caught");
    }

    announce("chrono monotonic");
    {
        auto t0 = std::chrono::steady_clock::now();
        volatile double acc = 0;
        for (int i = 1; i < 400000; i++) acc += 1.0 / i;
        auto t1 = std::chrono::steady_clock::now();
        if (t1 >= t0) ok();
        else no("steady_clock went backwards");
    }

    announce("regex");
    {
        // ★ The hyphen class is deliberate. This first read `(\w+)@` and expected to capture
        // "nyx-user", which FAILED -- correctly, because \w is [A-Za-z0-9_] and stops at the
        // hyphen, so the engine captured "user". The library was right and the test was wrong.
        // Worth keeping as a note: on a bring-up like this the reflex is to suspect the new
        // component, and here that reflex would have sent me digging through libc++'s regex.
        std::regex re(R"(([\w-]+)@(\w+)\.nyx)");
        std::smatch m;
        std::string subject = "mail nyx-user@example.nyx now";
        if (std::regex_search(subject, m, re) && m[1] == "nyx-user" && m[2] == "example") ok();
        else no("regex did not match");
    }

    // ── filesystem, split into one probe per operation ───────────────────────────────────────
    // ★ This was ONE probe bundling create+write+size+iterate+remove, and it reported
    // "create/size/iterate/remove mismatch" on Nyx -- a verdict naming five hypotheses at once,
    // which is exactly the mistake already written down as a rule: a probe's verdict must not
    // depend on syscalls other than the one under test. Split, and each step now prints the
    // error_code so a failure names a REASON rather than a stage.
    {
        auto root = scratch_root();
        // ★ Printed because the root CHOICE is itself a suspect. scratch_root() falls back to
        // "/tmp" when /mnt/nvme is not visible, and Nyx has no /tmp -- so a wrong answer here
        // makes every step below fail for a reason that has nothing to do with libc++.
        std::printf("cxx: (filesystem root = %s)\n", root.c_str());
        std::fflush(stdout);

        std::error_code ec;
        auto dir = root / "cxxscratch";
        auto f   = dir / "note.txt";
        std::filesystem::remove_all(dir, ec);   // best-effort cleanup from a previous run

        announce("fs: create_directories");
        ec.clear();
        std::filesystem::create_directories(dir, ec);
        bool made = std::filesystem::is_directory(dir, ec);
        if (made) ok(); else no(ec ? ec.message().c_str() : "not a directory afterwards");

        announce("fs: write + file_size");
        ec.clear();
        { std::ofstream out(f); out << "hello filesystem" << std::endl; }
        auto sz = std::filesystem::file_size(f, ec);
        // 16 chars + '\n'. An exact match matters: a short count would mean a lost flush, and a
        // zero would mean the write never landed at all.
        if (!ec && sz == 17) ok();
        else if (ec) no(ec.message().c_str());
        else { std::printf("(size=%llu) ", (unsigned long long)sz); no("wrong size"); }

        announce("fs: directory_iterator");
        ec.clear();
        int seen = 0, entries = 0;
        for (const auto &e : std::filesystem::directory_iterator(dir, ec)) {
            entries++;
            if (e.path().filename() == "note.txt") seen++;
        }
        if (!ec && seen == 1) ok();
        else if (ec) no(ec.message().c_str());
        else { std::printf("(entries=%d) ", entries); no("note.txt not listed"); }

        announce("fs: remove_all");
        ec.clear();
        std::filesystem::remove_all(dir, ec);
        bool gone = !std::filesystem::exists(dir, ec);
        if (gone) ok(); else no(ec ? ec.message().c_str() : "directory still exists");
    }

    // ★ Last on purpose: threads are the probe most likely to hang rather than fail, and
    // everything above must have already reported before that can happen.
    announce("thread + mutex");
    {
        std::mutex mu;
        int counter = 0;
        std::vector<std::thread> ts;
        for (int i = 0; i < 4; i++)
            ts.emplace_back([&] { for (int j = 0; j < 1000; j++) { std::lock_guard<std::mutex> lk(mu); counter++; } });
        for (auto &t : ts) t.join();
        // 4000 exactly proves the mutex actually excluded; a torn count would be less.
        if (counter == 4000) ok();
        else no("lost updates — mutex not excluding");
    }

    std::printf("=== SUMMARY pass=%d fail=%d ===\n", n_pass, n_fail);
    std::fflush(stdout);
    return n_fail == 0 ? 0 : 1;
}
