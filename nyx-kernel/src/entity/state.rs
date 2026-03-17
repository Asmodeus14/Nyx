use core::sync::atomic::{AtomicU32, Ordering};

// We use AtomicU32 to store f32 values safely across threads by transmuting them.
pub struct NyxState {
    pub energy: AtomicU32,     // Driven by CPU load / Thread switching
    pub entropy: AtomicU32,    // Driven by Memory usage & Filesystem I/O
    pub stability: AtomicU32,  // Driven by Uptime and undisturbed idling
    pub curiosity: AtomicU32,  // Driven by User input (Mouse/Keyboard events)
}

impl NyxState {
    pub const fn new() -> Self {
        Self {
            energy: AtomicU32::new(0),
            entropy: AtomicU32::new(0),
            stability: AtomicU32::new(0),
            curiosity: AtomicU32::new(0),
        }
    }

    // Helper to read state as floats
    pub fn get_energy(&self) -> f32 { f32::from_bits(self.energy.load(Ordering::Relaxed)) }
    pub fn get_entropy(&self) -> f32 { f32::from_bits(self.entropy.load(Ordering::Relaxed)) }
    pub fn get_stability(&self) -> f32 { f32::from_bits(self.stability.load(Ordering::Relaxed)) }
    pub fn get_curiosity(&self) -> f32 { f32::from_bits(self.curiosity.load(Ordering::Relaxed)) }

    // Helper to update state
    pub fn add_energy(&self, amount: f32) {
        let current = self.get_energy();
        let new_val = (current + amount).clamp(0.0, 100.0);
        self.energy.store(new_val.to_bits(), Ordering::Relaxed);
    }
    
    pub fn add_entropy(&self, amount: f32) {
        let current = self.get_entropy();
        let new_val = (current + amount).clamp(0.0, 100.0);
        self.entropy.store(new_val.to_bits(), Ordering::Relaxed);
    }
}

pub static ENTITY_STATE: NyxState = NyxState::new();

// This function will be called 10 times a second by the kernel background thread
pub fn evolve_state() {
    // 1. Natural Decay: Everything slowly calms down over time
    let current_energy = ENTITY_STATE.get_energy();
    if current_energy > 0.0 {
        ENTITY_STATE.energy.store((current_energy - 0.01).max(0.0).to_bits(), Ordering::Relaxed);
    }
    
    // 2. Stability grows naturally over time if left alone
    let current_stability = ENTITY_STATE.get_stability();
    ENTITY_STATE.stability.store((current_stability + 0.005).min(100.0).to_bits(), Ordering::Relaxed);
    
    // TODO: In Step 2, we will inject QCLang probability matrices here 
    // to organically mutate these values instead of using linear math.
}