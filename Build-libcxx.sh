#!/usr/bin/env bash
# Build libc++ + libc++abi + libunwind against Nyx's musl. Phase 6, gate 2 (the library half).
#
# Prerequisites: ./Build-musl.sh has produced vendor/musl/lib/libc.a, and $HOME/llvm-src holds a
# sparse llvmorg-18.1.3 checkout of {libcxx,libcxxabi,libunwind,runtimes,cmake,llvm/cmake}.
#
# ── Why a "runtimes" build, not a full LLVM build ────────────────────────────────────────────
# libcxx/libcxxabi/libunwind are standalone runtimes needing only a working C compiler and a
# libc's HEADERS. We are not building clang; the host's clang 18.1.3 is the compiler.
#
# ── Why libc++ and not libstdc++ ─────────────────────────────────────────────────────────────
# Measured, not assumed. libstdc++'s headers are hard-bound to glibc: bits/os_defines.h uses
# __GLIBC_PREREQ, which is undefined under musl, so every `#if` becomes a syntax error, and
# bits/c++locale.h wants glibc's __locale_t. That is a COMPILE-time wall no link-time shim can
# reach -- unlike libsupc++, which we did get working on musl with four small shims. libc++
# supports musl as a first-class configuration (LIBCXX_HAS_MUSL_LIBC).
#
# ── Version pin ──────────────────────────────────────────────────────────────────────────────
# libc++ is matched to clang 18.1.3 exactly. libc++ and the compiler are tightly coupled through
# __has_builtin feature tests and ABI macros, and a skew here produces failures that read like
# musl-porting problems rather than a mismatched toolchain.
set -e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
M="$ROOT/vendor/musl"
SRC="${LLVM_SRC:-$HOME/llvm-src}"
B="$HOME/libcxx-build"
PREFIX="$HOME/nyx-cxx"

[ -f "$M/lib/libc.a" ] || { echo "[libcxx] musl libc.a missing — run ./Build-musl.sh" >&2; exit 1; }
[ -d "$SRC/libcxx" ]   || { echo "[libcxx] LLVM runtimes source missing at $SRC" >&2; exit 1; }

# clang's own resource headers (stddef.h, stdarg.h, ...). -nostdinc drops these too, and musl's
# headers depend on them, so they go back FIRST -- the same ordering rule as tests/helloc/build.sh.
CLANGINC="$(clang -print-resource-dir)/include"

# ── Kernel UAPI headers ──────────────────────────────────────────────────────────────────────
# libc++ includes <linux/futex.h> (atomic.cpp) and <linux/random.h> (random.cpp). musl does not
# ship those: they are the KERNEL's uapi, not a libc's, and on glibc systems they come from the
# separate linux-libc-dev package. Using them is correct rather than a fudge -- Nyx's syscalls are
# deliberately Linux-numbered and Linux-shaped, so the kernel ABI those headers describe IS Nyx's.
# (Checked: Nyx's futex arm already strips FUTEX_PRIVATE_FLAG, so FUTEX_WAIT_PRIVATE works.)
#
# A private symlink FARM rather than `-isystem /usr/include`: the latter would put glibc's stdio.h,
# stdlib.h and friends back on the path, silently undoing -nostdinc and producing a libc++ built
# against a libc that will never be underneath it. Only the three kernel dirs are exposed.
UAPI="$HOME/nyx-uapi"
rm -rf "$UAPI" && mkdir -p "$UAPI"
ln -s /usr/include/linux                 "$UAPI/linux"
ln -s /usr/include/asm-generic           "$UAPI/asm-generic"
ln -s /usr/include/x86_64-linux-gnu/asm  "$UAPI/asm"

# $UAPI goes LAST so musl's headers win any name collision.
MUSL_INC="-nostdinc -isystem $CLANGINC -isystem $M/include -isystem $M/obj/include -isystem $M/arch/x86_64 -isystem $M/arch/generic -isystem $UAPI"

rm -rf "$B" "$PREFIX"

# CMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY: cmake's compiler probe would otherwise try to LINK
# an executable, which needs crt1.o and libc.a and a load address -- none of which apply while
# building a library. Compiling is all the probe actually needs to prove.
#
# ★ LLVM_INCLUDE_TESTS=OFF is REQUIRED here and is not implied by the three per-runtime
# *_INCLUDE_TESTS=OFF flags. runtimes/CMakeLists.txt guards `add_subdirectory(.../utils/llvm-lit)`
# on the umbrella LLVM_INCLUDE_TESTS, so without it the configure dies looking for llvm-lit, which
# a sparse checkout of just the runtimes does not contain.
cmake -G Ninja -S "$SRC/runtimes" -B "$B" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DCMAKE_C_COMPILER=clang \
  -DCMAKE_CXX_COMPILER=clang++ \
  -DCMAKE_C_FLAGS="$MUSL_INC" \
  -DCMAKE_CXX_FLAGS="$MUSL_INC" \
  -DLLVM_ENABLE_RUNTIMES="libcxx;libcxxabi;libunwind" \
  -DLIBCXX_HAS_MUSL_LIBC=ON \
  -DLIBCXX_ENABLE_SHARED=OFF \
  -DLIBCXXABI_ENABLE_SHARED=OFF \
  -DLIBUNWIND_ENABLE_SHARED=OFF \
  -DLIBCXX_CXX_ABI=libcxxabi \
  -DLIBCXXABI_USE_LLVM_UNWINDER=ON \
  -DLIBCXXABI_ENABLE_STATIC_UNWINDER=ON \
  -DLIBCXX_USE_COMPILER_RT=OFF \
  -DLIBCXXABI_USE_COMPILER_RT=OFF \
  -DLIBCXX_ENABLE_STATIC_ABI_LIBRARY=ON \
  -DLIBCXX_INCLUDE_BENCHMARKS=OFF \
  -DLIBCXX_INCLUDE_TESTS=OFF \
  -DLIBCXXABI_INCLUDE_TESTS=OFF \
  -DLIBUNWIND_INCLUDE_TESTS=OFF \
  -DLLVM_INCLUDE_TESTS=OFF \
  -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY \
  > /tmp/cxx_cfg.log 2>&1 || { echo "[libcxx] CONFIGURE FAILED"; tail -40 /tmp/cxx_cfg.log; exit 1; }
echo "[libcxx] configured."

ninja -C "$B" -j"$(nproc)" > /tmp/cxx_build.log 2>&1 \
  || { echo "[libcxx] BUILD FAILED"; tail -50 /tmp/cxx_build.log; exit 1; }
ninja -C "$B" install > /tmp/cxx_inst.log 2>&1 \
  || { echo "[libcxx] INSTALL FAILED"; tail -30 /tmp/cxx_inst.log; exit 1; }

echo "[libcxx] built + installed to $PREFIX"
find "$PREFIX" -name '*.a' | sed 's/^/    /'
