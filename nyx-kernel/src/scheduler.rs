use alloc::vec::Vec;
use alloc::boxed::Box;

// A simple Linear Congruential Generator for "Quantum" randomness
struct QuantumRng {
    seed: u64,
}

impl QuantumRng {
    fn new(seed: u64) -> Self {
        Self { seed }
    }

    fn next(&mut self) -> u64 {
        // Parameters typically used in simple LCGs
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.seed
    }

    fn next_limit(&mut self, limit: usize) -> usize {
        (self.next() as usize) % limit
    }
}

pub struct Task {
    pub id: usize,
    pub handler: Box<dyn FnMut() + Send>,
    pub active: bool,
    pub tickets: usize, // "Quantum Weight" - more tickets = higher chance to run
}

pub struct Scheduler {
    tasks: Vec<Task>,
    current_id: usize,
    rng: QuantumRng,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            current_id: 0,
            rng: QuantumRng::new(12345), // Initial seed
        }
    }

    /// Spawns a task with a specific "Quantum Weight" (tickets).
    /// Higher tickets = Runs more often.
    /// - System Critical (Quantum Control): 100 tickets
    /// - UI (Classical Shell): 20 tickets
    /// - Background (Maintenance): 5 tickets
    pub fn spawn(&mut self, tickets: usize, handler: impl FnMut() + Send + 'static) {
        let task = Task {
            id: self.current_id,
            handler: Box::new(handler),
            active: true,
            tickets,
        };
        self.tasks.push(task);
        self.current_id += 1;
    }

    pub fn run(&mut self) {
        if self.tasks.is_empty() { return; }

        // 1. Calculate total tickets in the system (The "Superposition")
        let total_tickets: usize = self.tasks.iter()
            .filter(|t| t.active)
            .map(|t| t.tickets)
            .sum();

        if total_tickets == 0 { return; }

        // 2. Collapse the Wave Function (Pick a Winner)
        let mut winning_ticket = self.rng.next_limit(total_tickets);

        // 3. Find which task holds the winning ticket
        let mut ticket_counter = 0;
        
        // We need to iterate mutably, but we can't borrow the whole vector while picking
        // So we use indices.
        let mut winner_idx = 0;
        
        for (i, task) in self.tasks.iter().enumerate() {
            if !task.active { continue; }
            
            ticket_counter += task.tickets;
            if ticket_counter > winning_ticket {
                winner_idx = i;
                break;
            }
        }

        // 4. Execute the Quantum Winner
        if let Some(task) = self.tasks.get_mut(winner_idx) {
            (task.handler)();
        }
    }
}