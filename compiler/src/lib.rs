// compiler/src/lib.rs - PHASE 2.1 INTEGRATION
pub mod lexer;
pub mod ast;
pub mod parser;
pub mod ir;
pub mod qir;
pub mod codegen;
pub mod semantics;
pub mod error;
pub mod simulator; // <--- Added: Simulator Module

use lexer::tokenize;
use parser::Parser;
use qir::builder::QirBuilder;
use qir::optimizer::QirOptimizer;
use qir::analysis::QirAnalyzer;
use semantics::SemanticAnalyzer;
use codegen::QASMGenerator;
use qir::QirModule;
use std::time::SystemTime;

pub const VERSION: &str = "0.6.0";

// Dynamic build info (requires chrono in Cargo.toml)
pub fn build_timestamp() -> String {
    let now = SystemTime::now();
    let dt = chrono::DateTime::<chrono::Utc>::from(now);
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

// Fetches hash from build.rs
pub fn git_commit_hash() -> String {
    env!("GIT_HASH").to_string()
}

// --- Return Structures ---

#[derive(Debug, Clone, Copy)]
pub struct CompileStats {
    pub qubits: usize,
    pub cbits: usize,
    pub gates: usize,
    pub measurements: usize,
}

impl CompileStats {
    pub fn new() -> Self {
        Self { qubits: 0, cbits: 0, gates: 0, measurements: 0 }
    }
    
    pub fn total_operations(&self) -> usize {
        self.gates + self.measurements
    }
}

impl Default for CompileStats {
    fn default() -> Self { Self::new() }
}

/// The result of a successful compilation.
/// Contains the QASM code, statistics, and the IR for simulation.
pub struct CompilationResult {
    pub qasm: String,
    pub stats: CompileStats,
    pub ir: QirModule, // <--- Exposed for Simulator
}

// --- Compiler Implementation ---

pub struct Compiler;

impl Compiler {
    pub fn new() -> Self {
        Self
    }
    
    pub fn version() -> &'static str {
        VERSION
    }
    
    pub fn capabilities() -> Vec<&'static str> {
        vec![
            "Phase 2.1: Quantum Runtime",
            "• Statevector Simulation (Local)",
            "• Execution of QIR modules",
            "Phase 1.5: QIR Optimizations",
            "• Dead Qubit Elimination",
            "• Gate Cancellation",
            "• QIR-to-QASM Generation",
            "• Type-safe intermediate representation",
            "• QIR analysis and verification",
            "Phase 1.4: Semantic Analyzer",
            "• Quantum ownership rules",
            "• Type Registry & Symbol Table",
            "Standard gates: H, X, Y, Z, CNOT",
            "Quantum control flow: qif, qfor",
        ]
    }
    
    // UPDATED: Returns CompilationResult instead of tuple
    pub fn compile_with_stats(source: &str, optimize: bool) -> Result<CompilationResult, Vec<String>> {
        // 1. LEXING
        let tokens = tokenize(source);
        
        // 2. PARSING
        let mut parser = Parser::new(tokens.into_iter(), source.to_string());
        let program = parser.parse_program();
        
        if !parser.errors.is_empty() {
            return Err(parser.errors.iter().map(|e| e.to_string()).collect());
        }
        
        // 3. SEMANTIC ANALYSIS
        let mut semantic_analyzer = SemanticAnalyzer::new();
        match semantic_analyzer.analyze_program(&program) {
            Ok(_) => {
                for warning in semantic_analyzer.get_warnings() {
                    eprintln!("Warning: {}", warning);
                }
            }
            Err(errors) => {
                return Err(errors.iter().map(|e| e.to_string()).collect());
            }
        }
        
        // 4. QIR GENERATION
        let mut qir_builder = QirBuilder::new();
        let mut qir_module = qir_builder.build_from_program(&program);
        
        // 5. QIR OPTIMIZATION
        if optimize {
            let optimizer = QirOptimizer::new(true);
            optimizer.optimize_module(&mut qir_module);
        }
        
        // 6. QIR ANALYSIS (Safety Check)
        let mut analyzer = QirAnalyzer::new();
        if !analyzer.analyze_module(&qir_module) {
            return Err(analyzer.get_errors().iter().map(|s| s.clone()).collect());
        }
        
        // 7. CODE GENERATION (OpenQASM)
        let mut qasm_generator = QASMGenerator::new();
        let qasm_code = qasm_generator.generate(&qir_module);
        
        // 8. STATS GATHERING
        let stats = CompileStats {
            qubits: qasm_generator.qubit_count(),
            cbits: qasm_generator.cbit_count(),
            gates: qasm_generator.gate_count(),
            measurements: qasm_generator.measurement_count(),
        };
        
        Ok(CompilationResult {
            qasm: qasm_code,
            stats,
            ir: qir_module, // Pass the IR out for the simulator
        })
    }
    
    // Helper for simple QASM string output
    pub fn compile(source: &str) -> Result<String, Vec<String>> {
        Self::compile_with_stats(source, true).map(|res| res.qasm)
    }
    
    // Helper for tests/diagnostics
    pub fn compile_with_diagnostics(source: &str) -> (Result<String, Vec<String>>, CompileStats) {
        match Self::compile_with_stats(source, true) {
            Ok(res) => (Ok(res.qasm), res.stats),
            Err(e) => (Err(e), CompileStats::default()),
        }
    }
}