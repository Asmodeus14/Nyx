// compiler/src/simulator.rs - FIXED VERSION
use crate::qir::{QirModule, QirOp, QirGate, QirValue};
use num_complex::Complex;
use std::f64::consts::SQRT_2;
use rand::Rng;

pub struct Simulator {
    state: Vec<Complex<f64>>,
    num_qubits: usize,
}

impl Simulator {
    pub fn new() -> Self {
        Self {
            state: vec![Complex::new(1.0, 0.0)],
            num_qubits: 0,
        }
    }

    pub fn execute(&mut self, module: &QirModule) -> Result<String, String> {
        let mut output = String::new();
        output.push_str("🚀 Simulation Log:\n");

        if let Some(func) = module.functions.iter().find(|f| f.name == "main") {
            // FIX: Use Control Flow Graph traversal
            let mut current_block_id = func.entry_block;
            let mut steps = 0;
            const MAX_STEPS: usize = 10000; // Guard against infinite loops

            loop {
                if steps > MAX_STEPS {
                    return Err("Simulation exceeded max steps (infinite loop detection)".to_string());
                }
                steps += 1;

                let block = func.blocks.get(&current_block_id)
                    .ok_or_else(|| format!("Invalid block ID: {:?}", current_block_id))?;

                let mut jumped = false;

                for op in &block.ops {
                    match op {
                        QirOp::AllocQubit { .. } => {
                            self.allocate_qubit();
                        }
                        QirOp::ApplyGate { gate, args, .. } => {
                            // FIX: Correct borrowing for arguments
                            self.apply_gate(gate, args)?;
                        }
                        QirOp::Measure { qubit, .. } => {
                            let result = self.measure(qubit.0);
                            output.push_str(&format!("  MEASURE q[{}] -> {}\n", qubit.0, result));
                        }
                        // --- Control Flow Handling ---
                        QirOp::Jump { target } => {
                            current_block_id = *target;
                            jumped = true;
                            break; // Stop processing this block, move to next
                        }
                        QirOp::Branch { cond: _, then_block, else_block: _ } => {
                            // Simplified: Always take 'then' branch for now (ignoring condition)
                            // In a full implementation, you'd check the 'cond' variable value
                            current_block_id = *then_block;
                            jumped = true;
                            break;
                        }
                        QirOp::Return { .. } => {
                            output.push_str("🏁 End of Simulation.\n");
                            return Ok(output);
                        }
                        _ => {}
                    }
                }

                // Implicit return if block ends with no jump
                if !jumped {
                    break;
                }
            }
        } else {
            return Err("No 'main' function found.".to_string());
        }

        output.push_str("🏁 End of Simulation.\n");
        Ok(output)
    }

    fn allocate_qubit(&mut self) {
        let old_len = self.state.len();
        let new_len = old_len * 2;
        let mut new_state = vec![Complex::new(0.0, 0.0); new_len];
        for i in 0..old_len {
            new_state[i] = self.state[i];
        }
        self.state = new_state;
        self.num_qubits += 1;
    }

    fn apply_gate(&mut self, gate: &QirGate, args: &[QirValue]) -> Result<(), String> {
        match gate {
            QirGate::H => {
                if let Some(QirValue::Qubit(qid)) = args.first() {
                    self.apply_h(qid.0);
                }
            }
            QirGate::X => {
                if let Some(QirValue::Qubit(qid)) = args.first() {
                    self.apply_x(qid.0);
                }
            }
            // FIX: Changed CX to CNOT to match your QirGate enum definition
            QirGate::CNOT => {
                if args.len() == 2 {
                    if let (QirValue::Qubit(c), QirValue::Qubit(t)) = (&args[0], &args[1]) {
                        self.apply_cx(c.0, t.0);
                    }
                }
            }
            // Handle cases where CX might be named differently or valid
            _ => return Err(format!("Simulator doesn't support gate {:?} yet", gate)),
        }
        Ok(())
    }

    // --- Math Kernels ---

    fn apply_h(&mut self, target: usize) {
        let size = self.state.len();
        let mut new_state = self.state.clone();
        for i in 0..size {
            if (i & (1 << target)) == 0 {
                let j = i | (1 << target);
                let a = self.state[i];
                let b = self.state[j];
                new_state[i] = (a + b) / SQRT_2;
                new_state[j] = (a - b) / SQRT_2;
            }
        }
        self.state = new_state;
    }

    fn apply_x(&mut self, target: usize) {
        let size = self.state.len();
        let mut new_state = vec![Complex::new(0.0, 0.0); size];
        for i in 0..size {
            let j = i ^ (1 << target);
            new_state[j] = self.state[i];
        }
        self.state = new_state;
    }

    fn apply_cx(&mut self, control: usize, target: usize) {
        let size = self.state.len();
        let mut new_state = self.state.clone();
        for i in 0..size {
            if (i & (1 << control)) != 0 {
                let j = i ^ (1 << target);
                if i < j { new_state.swap(i, j); }
            }
        }
        self.state = new_state;
    }

    fn measure(&mut self, target: usize) -> u8 {
        let mut prob_one = 0.0;
        for i in 0..self.state.len() {
            if (i & (1 << target)) != 0 {
                prob_one += self.state[i].norm_sqr();
            }
        }

        let mut rng = rand::thread_rng();
        let result = if rng.gen::<f64>() < prob_one { 1 } else { 0 };

        let prob = if result == 1 { prob_one } else { 1.0 - prob_one };
        if prob > 0.0 {
            let norm = 1.0 / prob.sqrt();
            for i in 0..self.state.len() {
                let bit_is_set = (i & (1 << target)) != 0;
                let bit_val = if bit_is_set { 1 } else { 0 };
                if bit_val == result {
                    self.state[i] = self.state[i] * norm;
                } else {
                    self.state[i] = Complex::new(0.0, 0.0);
                }
            }
        }
        result
    }
}