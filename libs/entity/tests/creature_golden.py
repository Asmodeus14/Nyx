# Regenerates `creature.golden`, the file `nyx-entity`'s cross-check test compares itself against:
#
#     python libs/entity/tests/creature_golden.py > libs/entity/tests/creature.golden
#     cargo test -p nyx-entity
#
# A line-by-line transcription of the JavaScript generator in
# Nyx-ui/design/ver3.0/parts/16-entity-creature.html.
#
# Checked in beside the golden it produces, because a golden nobody can regenerate is a golden
# nobody can update — the first time the design changes an archetype the only options would be to
# hand-edit 132 lines of 576 digits, or to delete the test.
#
# Transcribed from the JS, NOT from the Rust — the whole point is that it is an independent
# third reading, so an agreement between it and the Rust is evidence rather than a tautology.
# Python rather than Node only because this machine has no Node.
#
# `CORPUS` in `libs/entity/src/lib.rs` must stay identical to `SEEDS` below, and in the same order.
#
# JS semantics preserved deliberately:
#   * `x ^= x << 13` on a signed 32-bit int, `>>> 17` unsigned, and the closure returns `x >>> 0`.
#   * `arr[i] || d` is substitute-if-FALSEY, so it also fires when the value is 0 — and it yields
#     `d` for an out-of-range index, where a Rust slice would panic or need clamping.
#   * `Math.round` rounds half AWAY from zero for positives.
M = 0xFFFFFFFF
G = 24
OUT, SHADOW, BODY, LIGHT, ACC, LITE = 1, 2, 3, 4, 5, 6

PROFILE = {
    0: [2,4,5,5,3,3,5,6,7,7,7,7,7,6,5,4,3,2],
    1: [3,4,4,4,3,4,5,5,4,4,3,3,2,2,1,1,0,0],
    2: [1,2,3,4,5,6,7,8,8,7,6,5,4,3,2,1,0,0],
    3: [2,4,6,7,7,6,5,4,3,2,2,1,1,0,0,0,0,0],
    4: [1,2,3,3,3,4,4,4,4,4,3,3,3,2,2,1,0,0],
    5: [2,4,6,7,8,8,8,8,8,8,8,8,7,6,4,2,0,0],
}
TRAITS = ['Curious','Calm','Energetic','Focused','Quiet','Playful','Observant','Protective']


def s32(v):
    v &= M
    return v - (1 << 32) if v & 0x80000000 else v


def rng(seed):
    x = s32(seed)
    if x == 0:
        x = s32(0x9E3779B9)
    state = [x]

    def nxt():
        x = state[0]
        x = s32(x ^ s32((x << 13) & M))
        x = s32(x ^ ((x & M) >> 17))
        x = s32(x ^ s32((x << 5) & M))
        state[0] = x
        return x & M
    return nxt


def jround(v):
    # Math.round: half away from zero, for positives half up.
    import math
    return math.floor(v + 0.5)


def genome(v):
    r = rng(v)
    g = {'seed': v}
    g['archetype'] = r() % 6
    g['widthScale'] = [80, 100, 115][r() % 3]
    g['heightRows'] = [15, 18, 20][r() % 3]
    g['crest'] = r() % 4
    g['eye'] = r() % 4
    g['mantle'] = r() % 4
    g['marking'] = r() % 5
    g['gait'] = r() % 4
    pool = list(TRAITS)
    t = []
    for _ in range(3):
        t.append(pool.pop(r() % len(pool)))
    g['traits'] = t
    g['detail'] = r()
    return g


def cells(g, stage, inf=None):
    inf = inf or {}
    f = [0] * (G * G)

    def st(x, y, v):
        if 0 <= x < G and 0 <= y < G:
            f[y * G + x] = v

    def at(x, y):
        return 0 if (x < 0 or x >= G or y < 0 or y >= G) else f[y * G + x]

    def hwv(i, d):
        # `hw[i] || d`
        if i < 0 or i >= len(hw):
            return d
        return hw[i] or d

    prof = PROFILE[g['archetype']]
    hw = []
    for i in range(g['heightRows']):
        sv = prof[jround(i * (len(prof) - 1) / (g['heightRows'] - 1))]
        hw.append(min(8, jround(sv * g['widthScale'] / 100)))
    f0, l0 = 0, len(hw) - 1
    while f0 < l0 and not hw[f0]:
        f0 += 1
    while l0 > f0 and not hw[l0]:
        l0 -= 1
    hw = hw[f0:l0 + 1]
    rows = len(hw)
    top = 3 + ((20 - rows) >> 1)
    bot = top + rows - 1

    for i in range(rows):
        w, y = hw[i], top + i
        for d in range(w):
            st(11 - d, y, BODY)
            st(12 + d, y, BODY)

    if g['archetype'] == 5:
        for i in range(3, rows - 4):
            w, y = hw[i] - 3, top + i
            for d in range(w):
                st(11 - d, y, 0)
                st(12 + d, y, 0)

    for y in range(G):
        for x in range(G):
            if f[y * G + x] != BODY:
                continue
            if not at(x - 1, y) or not at(x + 1, y) or not at(x, y - 1) or not at(x, y + 1):
                st(x, y, OUT)
    for y in range(G):
        for x in range(G):
            if f[y * G + x] != BODY:
                continue
            if at(x - 1, y) == OUT or at(x, y - 1) == OUT or at(x - 1, y - 1) == OUT:
                st(x, y, LIGHT)
            elif at(x + 1, y) == OUT or at(x, y + 1) == OUT or at(x + 1, y + 1) == OUT:
                st(x, y, SHADOW)

    if g['archetype'] == 4 and stage >= 3:
        reach = 5 if stage >= 5 else 3
        for k in range(3):
            y = top + 3 + k * 4
            if y > bot:
                break
            x = 11 - hwv(y - top, 2)
            for d in range(1, reach + 1):
                fy = y + (d >> 1)
                st(x - d, fy, SHADOW if d == reach else BODY)
                st(23 - (x - d), fy, SHADOW if d == reach else BODY)
            if stage >= 5:
                st(x - reach, y + (reach >> 1), ACC)
                st(23 - (x - reach), y + (reach >> 1), ACC)

    eyi, lim = 1, max(2, jround(rows * 0.5))
    for i in range(1, lim):
        # JS reads past the end as `undefined`; `undefined > n` is false.
        if i < len(hw) and hw[i] > hw[eyi]:
            eyi = i
    ey, ehw = top + eyi, hw[eyi]
    wide = ehw >= 5
    ew = 2 if wide else 1
    off = max(ew, min(ehw - 1 - ew, 5))
    eyL, eyR = 11 - off - 1, 12 + off + 1

    def in_eye(px, py):
        return ey - 1 <= py <= ey + 2 and eyL <= px <= eyR

    def is_body(v):
        return v in (BODY, LIGHT, SHADOW)

    def mark(px, py):
        if py <= ey + 1 or in_eye(px, py):
            return
        if is_body(at(px, py)):
            st(px, py, ACC)
        if is_body(at(23 - px, py)):
            st(23 - px, py, ACC)

    if stage >= 2:
        n = stage - 1
        r2 = rng(s32(g['detail'] ^ ((g['marking'] * 2654435761) & M)))
        my = top + jround(rows * 0.62)
        if g['marking'] == 0:
            w = max(1, hwv(my - top, 3) - 2)
            for d in range(w):
                mark(11 - d, my)
                if stage >= 4:
                    mark(11 - d, my + 1)
        if g['marking'] == 1:
            for k in range(n):
                y = ey + 2 + k * 3
                w = hwv(y - top, 2)
                mark(12 - w, y)
                mark(12 - w, y + 1)
        if g['marking'] == 2:
            for k in range(n):
                y = ey + 2 + k * 2
                for d in range(1, 7, 2):
                    mark(11 - d, y)
        if g['marking'] == 3:
            for k in range(n + 2):
                mark(11, my - k)
        if g['marking'] == 4:
            for k in range(n * 2):
                x = 6 + r2() % 6
                y = ey + 2 + r2() % max(1, bot - ey - 1)
                mark(x, y)
                mark(x, y + 1)

    if stage >= 4 and g['crest'] > 0:
        cy, cw = top - 1, hw[0]
        if g['crest'] == 1:
            st(11, cy, ACC); st(12, cy, ACC); st(11, cy - 1, BODY); st(12, cy - 1, BODY)
        if g['crest'] == 2:
            st(11 - cw, cy, BODY); st(12 + cw, cy, BODY)
            st(11 - cw - 1, cy - 1, ACC); st(12 + cw + 1, cy - 1, ACC)
        if g['crest'] == 3:
            st(11, cy, ACC); st(12, cy, ACC)
            st(11 - 3, cy, BODY); st(12 + 3, cy, BODY)
            st(11 - 5, cy + 1, SHADOW); st(12 + 5, cy + 1, SHADOW)

    if stage >= 3 and g['mantle'] > 0:
        by = bot + 1
        if g['mantle'] == 1:
            for d in range(3):
                t = BODY if d < 1 else SHADOW
                st(11, by + d, t); st(12, by + d, t)
        if g['mantle'] == 2:
            for d in range(3):
                st(11 - d * 2, by, BODY); st(12 + d * 2, by, BODY)
                if d < 2:
                    st(11 - d * 2, by + 1, SHADOW); st(12 + d * 2, by + 1, SHADOW)
        if g['mantle'] == 3:
            for d in range(4):
                t = BODY if d < 2 else SHADOW
                st(11 - d, by + d, t); st(12 + d, by + d, t)

    lit = stage >= 2

    def eye(cx, w, h):
        for dy in range(h):
            for dx in range(w):
                st(cx + dx, ey + dy, ACC)
        if lit:
            st(cx, ey, LITE)

    if stage <= 1:
        st(11 - off, ey, ACC); st(12 + off, ey, ACC)
    elif g['eye'] == 0:
        eye(11 - off, ew, 2); eye(12 + off - ew + 1, ew, 2)
    elif g['eye'] == 1:
        eye(11 - off, ew, 1); eye(12 + off - ew + 1, ew, 1)
    elif g['eye'] == 2:
        eye(11 - off, 1, 2); eye(12 + off, 1, 2)
    else:
        eye(11 - off, 1, 1); eye(12 + off, 1, 1)
        if stage >= 4:
            st(11, ey + 2, ACC); st(12, ey + 2, ACC)

    if inf.get('luminance'):
        for y in range(G):
            for x in range(12):
                if at(x, y) == ACC and at(x, y - 1) == LIGHT and (x + y) % 2 == 0:
                    st(x, y - 1, LITE); st(23 - x, y - 1, LITE)
    if inf.get('crystal'):
        for y in range(top, top + rows, 3):
            w = hwv(y - top, 2)
            if at(12 - w + 1, y):
                st(12 - w + 1, y, LIGHT); st(11 + w - 1, y, LIGHT)
    if inf.get('flow'):
        by2 = top + rows - 2
        for d in range(3):
            if at(11 - d - 1, by2 - d):
                st(11 - d - 1, by2 - d, ACC); st(12 + d + 1, by2 - d, ACC)
    if inf.get('structure'):
        for y in range(top + 2, top + rows - 2, 4):
            for d in range(2, 6, 3):
                if at(11 - d, y):
                    st(11 - d, y, LIGHT)
                if at(12 + d, y):
                    st(12 + d, y, LIGHT)
    if inf.get('maturity'):
        if at(11, top + 1):
            st(11, top + 1, LITE); st(12, top + 1, LITE)

    return f, ey, top, rows


SEEDS = [0x7F3A91C2, 0x1A4490E1, 0x77B23C05, 0x5E09C1A8,
         0x00000001, 0xFFFFFFFF, 0xFEEDFACE, 0x0BADF00D,
         0x12345678, 0xABCDEF01, 0x9E3779B9, 0x11111111]
INFS = [
    {},
    {'luminance': True, 'crystal': True, 'flow': True, 'structure': True, 'maturity': True},
]

if __name__ == '__main__':
    lines = []
    for raw in SEEDS:
        g = genome(raw)
        lines.append('SEED %08X arch=%d w=%d h=%d crest=%d eye=%d mantle=%d marking=%d gait=%d detail=%u'
                     % (raw, g['archetype'], g['widthScale'], g['heightRows'], g['crest'],
                        g['eye'], g['mantle'], g['marking'], g['gait'], g['detail']))
        for stage in range(1, 6):
            for ii, inf in enumerate(INFS):
                f, ey, top, rows = cells(g, stage, inf)
                lines.append('S%d I%d ey=%d top=%d rows=%d %s'
                             % (stage, ii, ey, top, rows, ''.join(str(c) for c in f)))
    # Explicit LF. This file is compared against Rust's own output; a CRLF golden would differ
    # from it by an invisible byte on every single line.
    import sys, io as _io
    out = _io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', newline='\n')
    out.write('\n'.join(lines) + '\n')
    out.flush()
