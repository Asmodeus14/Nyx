// src/qir/mod.rs - COMPLETE FIXED VERSION
pub mod types;
pub mod operations;
pub mod builder;
pub mod optimizer;
pub mod analysis;

// Re-export public types
pub use types::{
    QubitId, CbitId, BlockId, TempId, QirType, QirParam, 
    QirValue, BitState
};
pub use operations::{QirGate, QirOp};
pub use builder::QirBuilder;
pub use optimizer::QirOptimizer;
pub use analysis::QirAnalyzer;

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub struct QirModule {
    pub name: String,
    pub version: String,
    pub functions: Vec<QirFunction>,
    pub global_qubits: Vec<QubitId>,
    pub global_cbits: Vec<CbitId>,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QirFunction {
    pub name: String,
    pub params: Vec<QirParam>,
    pub return_type: QirType,
    pub entry_block: BlockId,
    pub blocks: HashMap<BlockId, QirBlock>,
    pub current_block: BlockId,
    pub next_block_id: usize,
    pub next_qubit_id: usize,
    pub next_cbit_id: usize,
    pub next_temp_id: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QirBlock {
    pub id: BlockId,
    pub ops: Vec<QirOp>,
    pub predecessors: Vec<BlockId>,
    pub successors: Vec<BlockId>,
    pub live_qubits: HashSet<QubitId>,
    pub live_cbits: HashSet<CbitId>,
}

impl QirModule {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            functions: Vec::new(),
            global_qubits: Vec::new(),
            global_cbits: Vec::new(),
            metadata: HashMap::new(),
        }
    }
    
    pub fn add_function(&mut self, func: QirFunction) {
        self.functions.push(func);
    }
    
    pub fn add_global_qubit(&mut self) -> QubitId {
        let id = self.global_qubits.len();
        let qubit_id = QubitId::new(id);
        self.global_qubits.push(qubit_id);
        qubit_id
    }
    
    pub fn add_global_cbit(&mut self) -> CbitId {
        let id = self.global_cbits.len();
        let cbit_id = CbitId::new(id);
        self.global_cbits.push(cbit_id);
        cbit_id
    }

    // --- Added Statistics Methods ---

    pub fn qubit_count(&self) -> usize {
        let locals: usize = self.functions.iter().map(|f| f.next_qubit_id).sum();
        self.global_qubits.len() + locals
    }

    pub fn gate_count(&self) -> usize {
        self.functions.iter()
            .flat_map(|f| f.blocks.values())
            .flat_map(|b| b.ops.iter())
            .filter(|op| matches!(op, QirOp::ApplyGate { .. }))
            .count()
    }

    pub fn measurement_count(&self) -> usize {
        self.functions.iter()
            .flat_map(|f| f.blocks.values())
            .flat_map(|b| b.ops.iter())
            .filter(|op| matches!(op, QirOp::Measure { .. }))
            .count()
    }
}

impl QirFunction {
    pub fn new(name: &str, params: Vec<QirParam>, return_type: QirType) -> Self {
        let entry_block = BlockId::new(0);
        let mut blocks = HashMap::new();
        
        blocks.insert(entry_block, QirBlock {
            id: entry_block,
            ops: Vec::new(),
            predecessors: Vec::new(),
            successors: Vec::new(),
            live_qubits: HashSet::new(),
            live_cbits: HashSet::new(),
        });
        
        Self {
            name: name.to_string(),
            params,
            return_type,
            entry_block,
            blocks,
            current_block: entry_block,
            next_block_id: 1,
            next_qubit_id: 0,
            next_cbit_id: 0,
            next_temp_id: 0,
        }
    }
    
    pub fn create_block(&mut self) -> BlockId {
        let id = BlockId::new(self.next_block_id);
        self.next_block_id += 1;
        
        self.blocks.insert(id, QirBlock {
            id,
            ops: Vec::new(),
            predecessors: Vec::new(),
            successors: Vec::new(),
            live_qubits: HashSet::new(),
            live_cbits: HashSet::new(),
        });
        
        id
    }
    
    pub fn switch_to_block(&mut self, block_id: BlockId) {
        self.current_block = block_id;
    }
    
    pub fn get_current_block_mut(&mut self) -> &mut QirBlock {
        self.blocks.get_mut(&self.current_block).unwrap()
    }
    
    pub fn add_op(&mut self, op: QirOp) {
        let block = self.get_current_block_mut();
        block.ops.push(op);
    }
    
    pub fn add_jump(&mut self, target: BlockId) {
        let current = self.current_block;
        let target_block = self.blocks.get_mut(&target).unwrap();
        target_block.predecessors.push(current);
        
        let current_block = self.blocks.get_mut(&current).unwrap();
        current_block.successors.push(target);
        
        self.add_op(QirOp::Jump { target });
    }
    
    pub fn add_branch(&mut self, cond: QirValue, then_block: BlockId, else_block: BlockId) {
        let current = self.current_block;
        
        // Add predecessors
        let then_block_ref = self.blocks.get_mut(&then_block).unwrap();
        then_block_ref.predecessors.push(current);
        
        let else_block_ref = self.blocks.get_mut(&else_block).unwrap();
        else_block_ref.predecessors.push(current);
        
        // Add successors to current block
        let current_block = self.blocks.get_mut(&current).unwrap();
        current_block.successors.push(then_block);
        current_block.successors.push(else_block);
        
        self.add_op(QirOp::Branch { 
            cond, 
            then_block, 
            else_block 
        });
    }
    
    pub fn allocate_qubit(&mut self) -> QubitId {
        let id = self.next_qubit_id;
        self.next_qubit_id += 1;
        QubitId::new(id)
    }
    
    pub fn allocate_cbit(&mut self) -> CbitId {
        let id = self.next_cbit_id;
        self.next_cbit_id += 1;
        CbitId::new(id)
    }
    
    pub fn allocate_temp(&mut self) -> TempId {
        let id = self.next_temp_id;
        self.next_temp_id += 1;
        TempId::new(id)
    }
    
    pub fn get_successors(&self, block_id: BlockId) -> Vec<BlockId> {
        self.blocks.get(&block_id)
            .map(|b| b.successors.clone())
            .unwrap_or_default()
    }
    
    pub fn get_predecessors(&self, block_id: BlockId) -> Vec<BlockId> {
        self.blocks.get(&block_id)
            .map(|b| b.predecessors.clone())
            .unwrap_or_default()
    }
}

impl QirBlock {
    pub fn add_op(&mut self, op: QirOp) {
        self.ops.push(op);
    }
    
    pub fn add_live_qubit(&mut self, qubit: QubitId) {
        self.live_qubits.insert(qubit);
    }
    
    pub fn remove_live_qubit(&mut self, qubit: &QubitId) {
        self.live_qubits.remove(qubit);
    }
    
    pub fn add_live_cbit(&mut self, cbit: CbitId) {
        self.live_cbits.insert(cbit);
    }
    
    pub fn remove_live_cbit(&mut self, cbit: &CbitId) {
        self.live_cbits.remove(cbit);
    }
    
    pub fn get_terminator(&self) -> Option<&QirOp> {
        self.ops.last()
    }
    
    pub fn is_terminated(&self) -> bool {
        match self.ops.last() {
            Some(QirOp::Jump { .. }) | Some(QirOp::Branch { .. }) | Some(QirOp::Return { .. }) => true,
            _ => false,
        }
    }
}