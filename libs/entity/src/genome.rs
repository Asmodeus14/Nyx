//! Seed, deterministic expansion, genome, traits and evolution stage.
//!
//! The gene draw order in [`Genome::from_seed`] is part of the on-disk format.
//! Changing it does not break `entity.bin` — it silently mutates every Entity
//! that has ever existed. Add new genes at the END, never in the middle.

/// `xorshift32`. Bit-identical to the JavaScript reference generator, which is
/// what lets a seed rendered in the design document and the same seed rendered
/// on the machine produce the same creature, cell for cell.
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng(if seed == 0 { 0x9E37_79B9 } else { seed })
    }
    pub fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    fn pick(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

// ─────────────────────────────────────────────────────────────────────────
// SEED
// ─────────────────────────────────────────────────────────────────────────

/// 32 bits, struck once at first boot and never again.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Seed(pub u32);

impl Seed {
    /// Mixed at first boot. Three of the four inputs are machine-unique and
    /// stable; the clock is what stops two identical machines imaged from the
    /// same disk sharing a creature.
    pub fn strike(rtc_unix: u64, cpu_brand: &[u8], nvme_serial: &[u8], mac: [u8; 6]) -> Seed {
        let mut h: u32 = 0x811C_9DC5; // FNV-1a offset basis
        let fold = |bytes: &[u8], h: &mut u32| {
            for b in bytes {
                *h ^= *b as u32;
                *h = h.wrapping_mul(0x0100_0193);
            }
        };
        fold(&rtc_unix.to_le_bytes(), &mut h);
        fold(cpu_brand, &mut h);
        fold(nvme_serial, &mut h);
        fold(&mac, &mut h);
        Seed(if h == 0 { 0x9E37_79B9 } else { h })
    }

    /// `NX-7F3A-91C2`
    pub fn format(&self) -> [u8; 12] {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut out = *b"NX-0000-0000";
        let v = self.0;
        for i in 0..8 {
            let nib = ((v >> (28 - i * 4)) & 0xF) as usize;
            out[if i < 4 { 3 + i } else { 4 + i }] = HEX[nib];
        }
        out
    }

    /// Parses `NX-7F3A-91C2`, or any string whose last eight hex digits are the
    /// seed. Non-hex characters are ignored, which makes it paste-tolerant.
    pub fn parse(s: &[u8]) -> Option<Seed> {
        let mut v: u32 = 0;
        let mut n = 0usize;
        for &c in s {
            let d = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => continue,
            };
            v = (v << 4) | d as u32;
            n += 1;
        }
        if n == 0 { None } else { Some(Seed(v)) }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// EVOLUTION
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Stage {
    Genesis = 1,
    Awakening = 2,
    Adaptation = 3,
    Intelligence = 4,
    Mature = 5,
}

impl Stage {
    /// Uptime hours, not usage and not a score. An Entity nobody looks at
    /// develops at exactly the same rate as one watched daily.
    const GATES: [u32; 5] = [0, 8, 40, 150, 500];

    pub fn for_hours(h: u32) -> Stage {
        match h {
            _ if h >= Self::GATES[4] => Stage::Mature,
            _ if h >= Self::GATES[3] => Stage::Intelligence,
            _ if h >= Self::GATES[2] => Stage::Adaptation,
            _ if h >= Self::GATES[1] => Stage::Awakening,
            _ => Stage::Genesis,
        }
    }
    pub fn index(self) -> u8 { self as u8 }
    pub fn name(self) -> &'static str {
        ["Genesis", "Awakening", "Adaptation", "Intelligence", "Mature"][self as usize - 1]
    }
}

// ─────────────────────────────────────────────────────────────────────────
// PERSONALITY
// ─────────────────────────────────────────────────────────────────────────

pub const TRAITS: [&str; 8] = [
    "Curious", "Calm", "Energetic", "Focused",
    "Quiet", "Playful", "Observant", "Protective",
];

/// Traits do not touch appearance. They set timing — which is the part of a
/// creature a person reads without noticing they are reading it.
#[derive(Clone, Copy)]
pub struct Personality {
    pub traits: [u8; 3],
    /// Idle float period and blink interval, in 16 ms shell ticks.
    pub float_ticks: u16,
    pub blink_ticks: u16,
}

impl Personality {
    pub fn name(&self, i: usize) -> &'static str { TRAITS[self.traits[i] as usize] }
}

// ─────────────────────────────────────────────────────────────────────────
// ARCHETYPE + GENOME
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Archetype { Wanderer, Specter, Shard, Ember, Weaver, Void }

impl Archetype {
    pub fn from_u32(v: u32) -> Archetype {
        match v % 6 {
            0 => Archetype::Wanderer,
            1 => Archetype::Specter,
            2 => Archetype::Shard,
            3 => Archetype::Ember,
            4 => Archetype::Weaver,
            _ => Archetype::Void,
        }
    }
    pub fn name(self) -> &'static str {
        ["Wanderer", "Specter", "Shard", "Ember", "Weaver", "Void"][self as usize]
    }
}

/// Fixed at birth, complete from the first moment. Evolution changes how much
/// of this is *expressed*, never what it contains.
#[derive(Clone, Copy)]
pub struct Genome {
    pub seed: Seed,
    pub archetype: Archetype,
    pub width_scale: u16, // percent: 80 | 100 | 115
    pub height_rows: u8,  // 15 | 18 | 20
    pub crest: u8,        // 0 none  1 spine  2 horns  3 halo
    pub eye: u8,          // 0 pair  1 narrow 2 wide  3 triad
    pub mantle: u8,       // 0 none  1 wisp   2 fringe 3 trail
    pub marking: u8,      // 0 band  1 notch  2 lattice 3 spine 4 scatter
    pub gait: u8,
    pub personality: Personality,
    pub detail: u32, // reserved entropy, consumed by marking placement
}

impl Genome {
    /// Draw order is the format. Do not reorder.
    pub fn from_seed(seed: Seed) -> Genome {
        let mut r = Rng::new(seed.0);
        let archetype = Archetype::from_u32(r.pick(6));
        let width_scale = [80u16, 100, 115][r.pick(3) as usize];
        let height_rows = [15u8, 18, 20][r.pick(3) as usize];
        let crest = r.pick(4) as u8;
        let eye = r.pick(4) as u8;
        let mantle = r.pick(4) as u8;
        let marking = r.pick(5) as u8;
        let gait = r.pick(4) as u8;

        // Three distinct traits, drawn from a shrinking pool so the same trait
        // cannot come up twice. Mirrors the JS splice() exactly.
        let mut pool = [0u8, 1, 2, 3, 4, 5, 6, 7];
        let mut len = 8usize;
        let mut traits = [0u8; 3];
        for t in traits.iter_mut() {
            let i = (r.pick(len as u32)) as usize;
            *t = pool[i];
            for j in i..len - 1 { pool[j] = pool[j + 1]; }
            len -= 1;
        }
        let detail = r.next();

        // Gait sets the base timing; Energetic and Quiet bend it.
        let (mut fl, mut bl) = match gait {
            0 => (262u16, 437u16), // 4.2 s / 7.0 s at 16 ms
            1 => (212, 281),
            2 => (350, 562),
            _ => (181, 362),
        };
        for t in traits.iter() {
            match TRAITS[*t as usize] {
                "Energetic" => { fl = fl * 3 / 4; bl = bl * 2 / 3; }
                "Quiet" | "Calm" => { fl = fl * 5 / 4; bl = bl * 3 / 2; }
                _ => {}
            }
        }

        Genome {
            seed, archetype, width_scale, height_rows, crest, eye, mantle, marking, gait,
            personality: Personality { traits, float_ticks: fl, blink_ticks: bl },
            detail,
        }
    }
}
