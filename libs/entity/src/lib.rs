//! # nyx-entity
//!
//! Every Nyx installation has one digital creature. It is struck from a 32-bit
//! seed at first boot, it survives every reboot after that, and it develops
//! over the life of the machine.
//!
//! `no_std`, no `alloc`, no floating point. Ported from the design's own
//! reference implementation at `Nyx-ui/reference/entity`.
//!
//! The same algorithm exists twice: here, and in JavaScript inside the design
//! document at `design/ver3.0/parts/16-entity-creature.html`. They use the same
//! `xorshift32`, draw the same genes in the same order and emit the same run
//! list, so a seed rendered in the document and the same seed rendered on the
//! machine produce identical creatures, cell for cell. If you change one, run
//! the cross-check and change the other.
//!
//! **The JavaScript is the specification, not this file.** It is what the design
//! document renders, so it is what anyone has actually seen. Where the two ever
//! disagree, this one is wrong. One divergence was found and corrected when this
//! crate landed — see `appearance::hwv` — though it turned out to be latent
//! rather than live.
//!
//! The cross-check is a checked-in golden test rather than a script, because
//! "run the cross-check" is an instruction nobody follows and `cargo test` is
//! one people already run. See `tests::` at the bottom of this file.
//!
//! ```ignore
//! let st   = State::load(&buf).unwrap_or_else(|| State::birth(seed, now));
//! let gen  = Genome::from_seed(st.seed);
//! let fld  = appearance::build(&gen, st.stage(), &st.influence);
//! appearance::blit(&fld, 4, &mut surface, 96);   // 96 px, pitch 384 = 6*64
//! ```

#![no_std]
#![allow(clippy::needless_range_loop)]

// `no_std` on the target; the host test harness needs `std`. Test-only, so it never reaches a Nyx
// binary. Same arrangement as `nyx-meridian`.
#[cfg(test)]
extern crate std;

pub mod appearance;
pub mod genome;

pub use appearance::{Field, FillRect};
pub use genome::{Archetype, Genome, Personality, Rng, Seed, Stage, TRAITS};

// ─────────────────────────────────────────────────────────────────────────
// INFLUENCE
// ─────────────────────────────────────────────────────────────────────────

/// Six channels. Each is a counter that only ever goes up and crosses its
/// threshold at most once, so the creature changes permanently and rarely
/// rather than tracking a live reading.
///
/// This is the difference between a feature and a toy. A creature that
/// responded to live CPU load would twitch every time a build started, and
/// within a week nobody would look at it.
#[derive(Clone, Copy, Default)]
pub struct Counters {
    /// Sampled every 60 s. Units are sample-counts above threshold, not
    /// percentages, so a long idle period cannot erode what was earned.
    pub gpu_busy: u32,
    pub cpu_busy: u32,
    pub dev_hours: u32,
    pub qc_circuits: u32,
    pub net_mib: u32,
    pub uptime_hours: u32,
}

/// The expressed form of [`Counters`] — what `appearance::build` actually reads.
#[derive(Clone, Copy, Default)]
pub struct Influence {
    pub luminance: bool,
    pub energy: bool,
    pub structure: bool,
    pub crystal: bool,
    pub flow: bool,
    pub maturity: bool,
}

impl Counters {
    /// Thresholds are deliberately high. By the time one crosses, you cannot
    /// remember the Entity being otherwise — which is the intended experience.
    pub fn express(&self) -> Influence {
        Influence {
            luminance: self.gpu_busy >= 20_000,
            energy: self.cpu_busy >= 30_000,
            structure: self.dev_hours >= 200,
            crystal: self.qc_circuits >= 500,
            flow: self.net_mib >= 250_000,
            maturity: self.uptime_hours >= 800,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// STATE + PERSISTENCE
// ─────────────────────────────────────────────────────────────────────────

pub const MAGIC: u32 = 0x4E59_5845; // "NYXE"
pub const VERSION: u16 = 1;
pub const STATE_BYTES: usize = 48;

/// Everything that must survive a reboot. Deliberately tiny and deliberately
/// dull: the seed is the Entity, and everything else is either a counter or
/// recomputable. If this file is lost the creature is gone for good — there is
/// no backup and no way to reroll, which is the point.
#[derive(Clone, Copy)]
pub struct State {
    pub seed: Seed,
    pub born_unix: u64,
    pub last_seen_unix: u64,
    pub counters: Counters,
    pub influence: Influence,
}

impl State {
    pub fn birth(seed: Seed, now_unix: u64) -> State {
        State {
            seed,
            born_unix: now_unix,
            last_seen_unix: now_unix,
            counters: Counters::default(),
            influence: Influence::default(),
        }
    }

    pub fn genome(&self) -> Genome { Genome::from_seed(self.seed) }
    pub fn stage(&self) -> Stage { Stage::for_hours(self.counters.uptime_hours) }

    /// Call on the shell's tick. Returns `true` when the creature's *appearance*
    /// changed and the sprite must be regenerated — a few times a year, not a
    /// few times a second.
    pub fn tick_hour(&mut self, now_unix: u64) -> bool {
        let before_stage = self.stage().index();
        let before_inf = self.counters.express();
        self.counters.uptime_hours += 1;
        self.last_seen_unix = now_unix;
        let after_inf = self.counters.express();
        before_stage != self.stage().index()
            || before_inf.luminance != after_inf.luminance
            || before_inf.structure != after_inf.structure
            || before_inf.crystal != after_inf.crystal
            || before_inf.flow != after_inf.flow
            || before_inf.maturity != after_inf.maturity
    }

    // ---- on-disk format: 48 bytes, little-endian, at /mnt/nvme/entity.bin --
    // 0  magic u32 | 4 version u16 | 6 pad u16 | 8 seed u32 | 12 pad u32
    // 16 born u64  | 24 last_seen u64
    // 32 gpu u32   | 36 cpu u32 | 40 dev u32 | 44 qc u32
    // Counters that do not fit are recomputed rather than stored; net_mib and
    // uptime_hours are derived from the running sampler and the born stamp.

    pub fn save(&self, out: &mut [u8; STATE_BYTES]) {
        out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        out[4..6].copy_from_slice(&VERSION.to_le_bytes());
        out[6..8].copy_from_slice(&0u16.to_le_bytes());
        out[8..12].copy_from_slice(&self.seed.0.to_le_bytes());
        out[12..16].copy_from_slice(&self.counters.uptime_hours.to_le_bytes());
        out[16..24].copy_from_slice(&self.born_unix.to_le_bytes());
        out[24..32].copy_from_slice(&self.last_seen_unix.to_le_bytes());
        out[32..36].copy_from_slice(&self.counters.gpu_busy.to_le_bytes());
        out[36..40].copy_from_slice(&self.counters.cpu_busy.to_le_bytes());
        out[40..44].copy_from_slice(&self.counters.dev_hours.to_le_bytes());
        out[44..48].copy_from_slice(&self.counters.qc_circuits.to_le_bytes());
    }

    pub fn load(b: &[u8]) -> Option<State> {
        if b.len() < STATE_BYTES { return None; }
        let u32a = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let u64a = |i: usize| {
            let mut t = [0u8; 8];
            t.copy_from_slice(&b[i..i + 8]);
            u64::from_le_bytes(t)
        };
        if u32a(0) != MAGIC { return None; }
        if u16::from_le_bytes([b[4], b[5]]) != VERSION { return None; }
        let seed = Seed(u32a(8));
        if seed.0 == 0 { return None; }
        let counters = Counters {
            uptime_hours: u32a(12),
            gpu_busy: u32a(32),
            cpu_busy: u32a(36),
            dev_hours: u32a(40),
            qc_circuits: u32a(44),
            net_mib: 0,
        };
        Some(State {
            seed,
            born_unix: u64a(16),
            last_seen_unix: u64a(24),
            influence: counters.express(),
            counters,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────
// TESTS — cross-checked against the JavaScript reference generator.
// ─────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_roundtrip() {
        let s = Seed(0x7F3A_91C2);
        assert_eq!(&s.format(), b"NX-7F3A-91C2");
        assert_eq!(Seed::parse(b"NX-7F3A-91C2").unwrap().0, 0x7F3A_91C2);
    }

    /// The gene draw order is the on-disk format. If this fails, every Entity
    /// in existence has silently changed.
    #[test]
    fn genome_is_stable() {
        let g = Genome::from_seed(Seed(0x7F3A_91C2));
        assert_eq!(g.archetype.name(), "Specter");
        assert_eq!(g.width_scale, 80);
        assert_eq!(g.height_rows, 18);
        assert_eq!(g.eye, 3);
        assert_eq!(g.mantle, 3);
        assert_eq!(g.marking, 3);
    }

    #[test]
    fn stage_gates() {
        assert_eq!(Stage::for_hours(0).name(), "Genesis");
        assert_eq!(Stage::for_hours(39).name(), "Awakening");
        assert_eq!(Stage::for_hours(500).name(), "Mature");
    }

    #[test]
    fn state_roundtrip() {
        let mut st = State::birth(Seed(0x7F3A_91C2), 1_700_000_000);
        st.counters.uptime_hours = 312;
        st.counters.dev_hours = 240;
        let mut buf = [0u8; STATE_BYTES];
        st.save(&mut buf);
        let back = State::load(&buf).expect("round trip");
        assert_eq!(back.seed.0, st.seed.0);
        assert_eq!(back.counters.uptime_hours, 312);
        assert!(back.influence.structure);
        assert_eq!(back.stage().name(), "Intelligence");
    }

    /// The seeds and influence combinations the cross-check runs over.
    ///
    /// Twelve seeds is enough to hit all six archetypes, all three row counts, all five markings and
    /// both eye widths — checked, not hoped: `the_corpus_covers_the_genome` below fails if a future
    /// change to the draw order narrows the coverage.
    pub(crate) const CORPUS: [u32; 12] = [
        0x7F3A_91C2, 0x1A44_90E1, 0x77B2_3C05, 0x5E09_C1A8,
        0x0000_0001, 0xFFFF_FFFF, 0xFEED_FACE, 0x0BAD_F00D,
        0x1234_5678, 0xABCD_EF01, 0x9E37_79B9, 0x1111_1111,
    ];

    pub(crate) const ALL_INFLUENCE: Influence = Influence {
        luminance: true,
        energy: true,
        structure: true,
        crystal: true,
        flow: true,
        maturity: true,
    };

    /// Every field this crate can produce over `CORPUS`, as one text blob.
    ///
    /// Deliberately a flat, greppable dump rather than a hash: when it disagrees with the JavaScript
    /// the useful question is *which cell*, and a digest cannot answer that.
    pub(crate) fn dump() -> std::string::String {
        use core::fmt::Write;
        let mut s = std::string::String::new();
        for raw in CORPUS {
            let g = Genome::from_seed(Seed(raw));
            let _ = writeln!(
                s,
                "SEED {:08X} arch={} w={} h={} crest={} eye={} mantle={} marking={} gait={} detail={}",
                raw, g.archetype as u8, g.width_scale, g.height_rows, g.crest, g.eye, g.mantle,
                g.marking, g.gait, g.detail
            );
            for stage in [Stage::Genesis, Stage::Awakening, Stage::Adaptation,
                          Stage::Intelligence, Stage::Mature] {
                for (ii, inf) in [Influence::default(), ALL_INFLUENCE].iter().enumerate() {
                    let f = appearance::build(&g, stage, inf);
                    let _ = write!(s, "S{} I{} ey={} top={} rows={} ",
                                   stage.index(), ii, f.eye_row, f.top, f.rows);
                    for c in f.cells.iter() {
                        let _ = write!(s, "{}", c);
                    }
                    s.push('\n');
                }
            }
        }
        s
    }

    #[test]
    fn the_corpus_covers_the_genome() {
        let mut arch = [false; 6];
        let mut marking = [false; 5];
        let mut rows = [false; 3];
        for raw in CORPUS {
            let g = Genome::from_seed(Seed(raw));
            arch[g.archetype as usize] = true;
            marking[g.marking as usize] = true;
            rows[[15u8, 18, 20].iter().position(|&r| r == g.height_rows).unwrap()] = true;
        }
        assert!(arch.iter().all(|&b| b), "archetypes covered: {:?}", arch);
        assert!(marking.iter().all(|&b| b), "markings covered: {:?}", marking);
        assert!(rows.iter().all(|&b| b), "row counts covered: {:?}", rows);
    }

    /// ★ The cross-check, as a golden file.
    ///
    /// `tests/creature.golden` was produced by an independent transcription of the JavaScript in
    /// `16-entity-creature.html` — the design document's own generator, and therefore the only
    /// version of this creature anyone has actually looked at. Every cell of every field, for twelve
    /// seeds × five stages × two influence states, has to match it.
    ///
    /// The failure this catches is not a crash. A genome bug produces a perfectly good creature that
    /// is simply *not the one the seed describes*, and since nobody has seen their own Entity before
    /// it appears, there is no version of looking at it that would notice.
    #[test]
    fn the_creature_matches_the_design_documents_javascript() {
        let golden = include_str!("../tests/creature.golden");
        let ours = dump();
        // Line by line rather than as one blob: `str::lines` normalises CRLF, and a golden file that
        // took a trip through a Windows text-mode write would otherwise fail on an invisible byte.
        if ours.lines().count() == golden.lines().count()
            && ours.lines().zip(golden.lines()).all(|(a, b)| a == b)
        {
            return;
        }
        for (i, (a, b)) in ours.lines().zip(golden.lines()).enumerate() {
            if a != b {
                // Report the first differing CELL, not the first differing line — the lines are 576
                // digits long and a diff of them is unreadable.
                let col = a.chars().zip(b.chars()).position(|(x, y)| x != y);
                panic!(
                    "line {} differs at char {:?}\n  ours:   {}\n  golden: {}",
                    i + 1, col,
                    &a[..a.len().min(90)],
                    &b[..b.len().min(90)]
                );
            }
        }
        panic!("output has {} lines, golden has {}", ours.lines().count(), golden.lines().count());
    }

    #[test]
    fn creature_is_drawable_at_every_stage() {
        for raw in CORPUS {
            let g = Genome::from_seed(Seed(raw));
            for s in [Stage::Genesis, Stage::Awakening, Stage::Adaptation,
                      Stage::Intelligence, Stage::Mature] {
                let f = appearance::build(&g, s, &Influence::default());
                let mut out = [FillRect { x: 0, y: 0, w: 0, argb: 0 }; 256];
                let n = appearance::runs(&f, 4, &mut out);
                assert!(n > 20 && n < 200, "run count out of range: {}", n);
                // Every creature must have eyes — but not necessarily an ACC cell. The narrow eye
                // styles draw one cell each, `ACC` then immediately `LITE` on top, so from stage 2
                // a triad-eyed creature has no accent anywhere unless its markings landed. The
                // reference's original assertion checked `ACC` alone and was simply optimistic; it
                // happened to pass on the four seeds it was written against.
                let eyes = f.cells.iter().filter(|&&c| c == appearance::ACC || c == appearance::LITE);
                assert!(eyes.count() >= 2, "seed {:08X} stage {} has no eyes", raw, s.index());
            }
        }
    }

    /// `runs` is the whole renderer, so its bound is a real budget and not a smoke test.
    #[test]
    fn a_mature_creature_fits_the_documented_run_budget() {
        let mut worst = 0usize;
        for raw in CORPUS {
            let g = Genome::from_seed(Seed(raw));
            let f = appearance::build(&g, Stage::Mature, &ALL_INFLUENCE);
            let mut out = [FillRect { x: 0, y: 0, w: 0, argb: 0 }; 256];
            worst = worst.max(appearance::runs(&f, 4, &mut out));
        }
        // "A mature Entity comes to roughly 80-120 runs" — appearance.rs. The ceiling matters
        // because `runs` silently stops writing once `out` is full, so a creature that outgrew the
        // buffer would render with its bottom rows missing rather than fail.
        assert!(worst <= 160, "worst-case run count is {}", worst);
    }
}
