# Nyx OS — GPU UI Acceleration Plan

**Goal:** make the Nyx desktop UI dramatically smoother and hit high, stable frame rates by moving
the compositor and cursor off the CPU and onto the Gen9.5 GPU we now fully control (BLT engine +
the new RCS 3D/mini-GL engine). Today the compositor draws every pixel in software into one canvas
each frame, then a single BLT blit presents it, and the mouse cursor is re-composited by the CPU on
top. That is the bottleneck. This plan reuses the exact GPU capabilities already proven on bare
metal — no speculative hardware work; each phase is one testable feature per boot (boots are
expensive, real-HW only, no QEMU).

## Where we are (assets already proven on HW)

- **BLT engine:** `fill_rect`, `copy_rect`, fences, vsync (syscalls 501/502/512/513).
- **RCS 3D / mini-GL engine:** textured, perspective-correct, multi-mesh, **SSAA-antialiased**
  rendering; render-to-texture (offscreen Y-tiled RTs); bilinear sampling and a free box-downsample
  resolve. Exposed via syscalls 514–516 (`sys_gl_init` / `sys_gl_upload_mesh` / `sys_gl_render`).
- **Present path:** linear backbuffer at GVA `0x1400_0000` → scanout.
- **Shared memory + GGTT mapping** for zero-copy surfaces (`sys_gpu_map_shm`, 509).

The 3D engine's *screen-space textured-quad + blend* path is exactly what a modern compositor is.
The plan below turns that latent capability into an actual accelerated desktop.

---

## Phase U0 — Baseline & instrumentation (1 boot)

Measure before optimizing. Add an FPS/frame-time counter to the compositor (using `sys_get_uptime_ms`,
504) and a serial log of per-frame CPU time. Establish the current numbers (software canvas + BLT
present + CPU cursor) so every later phase has a concrete before/after.

- **Deliverable:** on-screen FPS + frame-time overlay; serial `[UI] frame N: cpu=Xms present=Yms`.
- **Gate:** numbers logged; no behavior change.

## Phase U1 — GPU hardware cursor (mouse off the CPU) (1–2 boots)

Move the mouse cursor to the **display controller's hardware cursor plane** — a dedicated overlay the
scanout engine composites in hardware every scan, independent of the framebuffer. This removes the
cursor from the compositor's redraw entirely: moving the mouse no longer dirties or re-blits any
window, and the cursor never tears or lags behind the frame.

- **What it is:** Gen9 pipe cursor registers (per pipe): `CUR_CTL` (enable + mode/size, e.g. 64×64
  ARGB), `CUR_BASE` (GGTT address of the cursor bitmap, triggers on write), `CUR_POS` (X/Y, signed).
  **SOURCE the exact offsets from the Intel PRM Vol2c display registers / i915 `intel_cursor.c` — do
  NOT guess** (a wrong display-register write can wedge scanout).
- **Work:** upload a 64×64 ARGB cursor bitmap to a GGTT page; kernel writes `CUR_CTL`/`CUR_BASE` once,
  then just `CUR_POS` on each mouse move (cheap MMIO). New syscalls: `sys_cursor_set_image` and
  `sys_cursor_move` (or drive `CUR_POS` directly from the PS/2 IRQ handler for zero-latency tracking).
- **Payoff:** immediate, very visible smoothness win; frees the compositor from cursor redraw.
- **Gate:** cursor moves glass-smooth with zero window redraws; compositor frame time drops.
- **Risk:** display-register correctness. Mitigate by sourcing offsets + keeping the software cursor
  as a fallback if the plane doesn't enable.

## Phase U2 — Alpha blending in the 3D pipeline (1 boot)

Turn on real **source-over alpha blending** in `BLEND_STATE` (currently disabled). This is the single
feature that unlocks the whole modern-compositor look: translucent windows, drop shadows, fades,
overlapping surfaces that composite correctly.

- **Work:** flip `BLEND_STATE_ENTRY` to enabled src-over (SRC_ALPHA / ONE_MINUS_SRC_ALPHA); source the
  blend-factor enums from genxml. Add a per-mesh "blend on/off" flag to the GL API.
- **Gate:** two overlapping textured quads with alpha show correct see-through compositing.

## Phase U3 — Screen-space 2D quad path (orthographic UI primitive) (1 boot)

Add an **orthographic, screen-space textured-quad** draw to the GL layer (no perspective camera): a
window is `draw_quad(x, y, w, h, texture, alpha)`. This is the atomic operation a GPU compositor and a
2D UI toolkit are built from. Reuses the proven textured-quad + (U2) blend path; only the transform
becomes a simple pixel-space ortho projection.

- **Work:** `sys_gl_draw_quad` (or a 2D-mode flag on the scene) that maps pixel rects → NDC; identity
  UVs; optional per-quad tint/alpha. Batch many quads into one submission.
- **Gate:** draw N translucent rounded rectangles at arbitrary positions in one GPU pass.

## Phase U4 — GPU compositor (windows as textures) (2 boots)

Rewrite the compositor's present path to be GPU-composited. Each window's pixels live in a shared-
memory surface (already supported via `sys_gpu_map_shm`); the compositor no longer software-copies
them into one canvas. Instead it issues one **textured quad per window** (U3) with blending (U2),
composited by the GPU directly into the backbuffer, then presents.

- **Boot 1:** composite the existing windows as GPU quads (opaque) — prove the model, measure FPS.
- **Boot 2:** add alpha/shadows + only re-composite on change (damage tracking); windows that didn't
  change cost nothing.
- **Payoff:** the big FPS win — CPU stops touching per-pixel window data; the GPU does the compositing
  it's designed for. Enables smooth window drag/resize/animation.
- **Gate:** dragging a window over others is smooth and correctly blended; CPU frame time collapses.

## Phase U5 — GPU-accelerated text (font atlas) (2 boots)

Text is currently CPU-drawn glyph by glyph. Upload the font as a **glyph-atlas texture** once; render
each glyph as a small textured quad (U3) sampling its atlas cell. GPU draws all the text of a frame in
one batched pass.

- **Boot 1:** atlas upload + render a string as textured quads (nearest or bilinear).
- **Boot 2:** subpixel/AA quality pass (reuse the SSAA/bilinear filtering already proven).
- **Gate:** crisp on-screen text via the GPU path; large text blocks no longer cost CPU time.
- **Note:** this is also groundwork any future browser/rich-UI needs (glyph rendering is universal).

## Phase U6 — Vsync-paced flip loop + hit a locked 60 FPS (1–2 boots)

Tie presentation to the panel's **60 Hz vertical refresh** (`sys_wait_vsync`, 513) with a clean
double-buffer flip so frames are tear-free and paced to exactly one flip per refresh (16.67 ms
budget). Combine with U4's damage tracking so the GPU only works when something changed, and a
frame-time governor that guarantees we never miss the vblank window.

- **Work:**
  - Front/back backbuffer flip; present exactly on vblank (no partial-frame tearing).
  - **Frame budget governor:** measure per-frame GPU+CPU time (U0 instrumentation); if a frame risks
    exceeding ~16 ms, split/queue work so the flip still lands on the next vblank rather than dropping
    to 30 FPS. Log dropped frames.
  - Idle path: when nothing is dirty, skip the whole render and just re-present (or hold), so an idle
    desktop costs ~0% and only wakes on input/animation.
  - Optional: read the panel's real refresh from the mode timings (source from the display
    controller / EDID) instead of assuming 60 Hz, so the pacing matches the actual scanout rate.
- **Gate:** steady, tear-free **60 FPS** under normal desktop use (dragging a window, scrolling);
  serial log shows frame time consistently < 16.67 ms and zero missed vblanks; idle desktop near 0%.
- **Why it matters:** this is the phase that converts all the earlier GPU offload into an actual FPS
  number — a locked 60 that matches the screen's refresh, with no tearing and no wasted work.

## Phase U7 — Window controls & UI visual correctness (2–3 boots)

Fix and complete the interactive window chrome so the desktop *looks and behaves* like a finished UI,
now that the compositor can cheaply redraw (U4) and animate (U8). This is the "make the visuals
correct" phase: **scrollbars, resize, maximize/fullscreen, minimize/restore**, and the smaller chrome
details that currently look rough.

- **Scope (each is a small, visible sub-milestone):**
  - **Scrollbars:** proper track + thumb, thumb size proportional to content, drag-to-scroll,
    click-in-track paging, hover/active states. Applies to terminal, explorer, settings, sysmon.
  - **Resize:** grab any window edge/corner; live resize with the compositor recompositing each frame
    (cheap now that windows are GPU textures); enforce min/max sizes; correct cursor shape per edge.
  - **Maximize / fullscreen:** snap a window to the full work area (respecting the taskbar) and to true
    fullscreen; restore to the previous rect. Animate the transition (U8).
  - **Minimize / restore:** hide to the taskbar and restore, with a smooth shrink/grow animation (U8);
    correct z-order and focus handling on restore.
  - **Chrome polish:** consistent title bars, close/min/max buttons with hover states, window borders,
    drop shadows (U2 alpha), rounded corners (already partially present via `draw_window_rounded`),
    focused-vs-unfocused window styling.
- **Work:** extend `window.rs` / `WINDOW_MANAGER` with per-window state (normal/maximized/minimized,
  saved-restore rect, min/max size, resize-edge hit-testing) and the compositor's input routing +
  per-window quad transform (position/scale) to reflect it. Reuse U3 quads + U2 blend for all chrome.
- **Gate:** every window can be scrolled, resized from any edge, maximized/fullscreened, and
  minimized/restored — all smooth (60 FPS, animated) and visually consistent across the built-in apps.
- **Note:** depends on U4 (windows-as-textures makes live resize/animation cheap) and U8 (animations).

## Phase U8 — Smoothness layer: animation & interpolation framework (1–2 boots)

A general, reusable mechanism that boosts *perceived* smoothness across the **entire** UI — not one
feature but the substrate every interaction rides on. The core idea: nothing in the UI should "jump."
State changes (open/close, focus, hover, minimize, maximize, scroll, drag-release) animate over a
short duration with easing, driven by the vsync clock, so motion is continuous at 60 FPS.

- **Work:**
  - A tiny **animation/tween engine** in the compositor: `animate(from → to, duration, easing)` driven
    by `sys_get_uptime_ms` / the vblank tick; supports position, size, opacity, and scroll offset.
  - Easing curves (ease-out/ease-in-out) so motion decelerates naturally instead of moving linearly.
  - **Motion/scroll smoothing:** momentum + interpolation for scrolling and window drag, so fast input
    still renders as smooth continuous motion rather than discrete jumps.
  - **Cursor & input smoothing:** with the U1 hardware cursor the pointer is already glass-smooth;
    here we also interpolate window-follow-drag and hover transitions to match.
  - Everything animating marks itself dirty for that frame only (feeds U4 damage + U6 pacing), so the
    smoothness costs GPU time only while motion is happening and returns to ~0% when it settles.
- **Gate:** opening/closing/minimizing/maximizing windows, focus changes, hover, and scrolling all
  visibly ease and interpolate at a locked 60 FPS; no popping or teleporting; idle cost returns to ~0%.
- **Why separate:** it's a horizontal capability U7's controls (and future apps) all consume, so it's
  cleaner to build the substrate once than to hand-animate each control.

---

## Sequencing rationale

- **U1 (hardware cursor) first** — biggest perceived-smoothness win for the least code, and directly
  answers "can the GPU draw the mouse?" (yes — the scanout hardware cursor plane).
- **U2→U3→U4** builds the compositor bottom-up: blending → the quad primitive → the full GPU
  compositor. Each is independently testable and visible.
- **U5 (text)** is polish + browser groundwork.
- **U6 (vsync/60 FPS)** turns the offload into a locked, tear-free 60 FPS number — do it once the
  compositor (U4) exists so there's real per-frame work to pace.
- **U8 (smoothness/animation substrate)** before **U7 (window controls)** so the controls animate for
  free; U7 is where scrollbars/resize/fullscreen/minimize become correct and polished on top of the
  fast, animated, 60 FPS foundation.

## Cross-cutting rules (unchanged from the 3D engine work)

- One new GPU capability per bare-metal boot; verify on the panel + sysmon boot-log.
- **Never guess display/render register or message encodings** — source from Intel PRM / i915 / Mesa
  genxml (a wrong write wastes a boot or wedges scanout). This applies especially to the U1 cursor
  registers and U2 blend enums.
- Keep a CPU/software fallback for every accelerated path (matches the existing GPU-fallback design).
- After each phase passes on HW, record the result + any gotcha in the project memory.

## Relationship to a future browser

U2–U5 (blend, quads, compositor, text) are precisely the layers a Servo/WebRender-style renderer
binds to. This plan makes the native UI fast *and* builds the exact compositing + text muscle a web
engine later needs, without committing to the multi-year browser-port cliff up front. The mini-GL
syscalls (514–516) are the seam a GPU-backed web renderer would target.
