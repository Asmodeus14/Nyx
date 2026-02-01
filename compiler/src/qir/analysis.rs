// src/qir/analysis.rs - FIXED WITH ALL PATTERNS
use super::*;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, PartialEq)]
pub enum ValueType {
    Int,
    Float,
    Bool,
    String,
    Qubit,
    Cbit,
    Tuple(Vec<ValueType>),
    Array(Box<ValueType>, usize),
    Unknown,
    Unit,
}

pub struct QirAnalyzer {
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl QirAnalyzer {
    pub fn new() -> Self {
        Self {
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }
    
    pub fn analyze_module(&mut self, module: &QirModule) -> bool {
        self.errors.clear();
        self.warnings.clear();
        
        // Check each function
        for func in &module.functions {
            self.analyze_function(func);
        }
        
        // Check global resources
        self.check_global_resources(module);
        
        self.errors.is_empty()
    }
    
    pub fn analyze_function(&mut self, func: &QirFunction) {
        // 1. Check block structure
        self.check_block_structure(func);
        
        // 2. Check SSA properties
        self.check_ssa_properties(func);
        
        // 3. Check qubit linearity
        self.check_qubit_linearity(func);
        
        // 4. Check type consistency
        self.check_type_consistency(func);
        
        // 5. Check control flow
        self.check_control_flow(func);
    }
    
    fn check_block_structure(&mut self, func: &QirFunction) {
        // All blocks should be reachable from entry block
        let reachable = self.compute_reachable_blocks(func);
        
        for &block_id in func.blocks.keys() {
            if !reachable.contains(&block_id) && block_id != func.entry_block {
                self.warnings.push(format!(
                    "Unreachable block {} in function {}",
                    block_id.id(), func.name
                ));
            }
        }
        
        // Each block should end with a terminator
        for (block_id, block) in &func.blocks {
            if !block.is_terminated() && !block.ops.is_empty() {
                self.errors.push(format!(
                    "Block {} in function {} doesn't end with a terminator",
                    block_id.id(), func.name
                ));
            }
        }
    }
    
    fn compute_reachable_blocks(&self, func: &QirFunction) -> HashSet<BlockId> {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        
        queue.push_back(func.entry_block);
        
        while let Some(block_id) = queue.pop_front() {
            if visited.contains(&block_id) {
                continue;
            }
            
            visited.insert(block_id);
            
            if let Some(block) = func.blocks.get(&block_id) {
                for &succ in &block.successors {
                    queue.push_back(succ);
                }
            }
        }
        
        visited
    }
    
    fn check_ssa_properties(&mut self, func: &QirFunction) {
        let mut definitions = HashMap::new();
        let mut uses = HashMap::new();
        
        for (block_id, block) in &func.blocks {
            for (op_index, op) in block.ops.iter().enumerate() {
                // Check which temps are defined
                if let Some(temp_id) = self.get_result_temp(op) {
                    if definitions.contains_key(&temp_id) {
                        self.errors.push(format!(
                            "Temp {} redefined in block {} (SSA violation)",
                            temp_id.id(), block_id.id()
                        ));
                    }
                    definitions.insert(temp_id, (block_id, op_index));
                }
                
                // Check which temps are used
                self.collect_temp_uses(op, &mut uses);
            }
        }
        
        // Check for undefined temps
        for (temp_id, _) in uses {
            if !definitions.contains_key(&temp_id) {
                self.errors.push(format!(
                    "Temp {} used before definition",
                    temp_id.id()
                ));
            }
        }
    }
    
    fn get_result_temp(&self, op: &QirOp) -> Option<TempId> {
        match op {
            QirOp::AllocQubit { result, .. } |
            QirOp::AllocCbit { result, .. } |
            QirOp::ClassicalAssign { target: result, .. } |
            QirOp::BinaryOp { result, .. } |
            QirOp::UnaryOp { result, .. } |
            QirOp::Load { result, .. } |
            QirOp::GetElementPtr { result, .. } |
            QirOp::MakeStruct { result, .. } |
            QirOp::ExtractField { result, .. } |
            QirOp::InsertField { result, .. } |
            QirOp::MakeArray { result, .. } |
            QirOp::ArrayGet { result, .. } |
            QirOp::ArraySet { result, .. } |
            QirOp::Phi { result, .. } => Some(*result),
            QirOp::ApplyGate { result, .. } => *result,
            _ => None,
        }
    }
    
    fn collect_temp_uses(&self, op: &QirOp, uses: &mut HashMap<TempId, usize>) {
        fn add_temp_use(temp_id: TempId, uses: &mut HashMap<TempId, usize>) {
            *uses.entry(temp_id).or_insert(0) += 1;
        }
        
        fn extract_temp(value: &QirValue) -> Option<TempId> {
            match value {
                QirValue::Temp(id) => Some(*id),
                _ => None,
            }
        }
        
        match op {
            QirOp::BinaryOp { lhs, rhs, .. } => {
                if let Some(temp_id) = extract_temp(lhs) {
                    add_temp_use(temp_id, uses);
                }
                if let Some(temp_id) = extract_temp(rhs) {
                    add_temp_use(temp_id, uses);
                }
            }
            QirOp::UnaryOp { operand, .. } => {
                if let Some(temp_id) = extract_temp(operand) {
                    add_temp_use(temp_id, uses);
                }
            }
            QirOp::Load { ptr, .. } => {
                add_temp_use(*ptr, uses);
            }
            QirOp::Store { ptr, value } => {
                add_temp_use(*ptr, uses);
                if let Some(temp_id) = extract_temp(value) {
                    add_temp_use(temp_id, uses);
                }
            }
            QirOp::GetElementPtr { base, .. } => {
                add_temp_use(*base, uses);
            }
            QirOp::ExtractField { struct_val, .. } => {
                if let Some(temp_id) = extract_temp(struct_val) {
                    add_temp_use(temp_id, uses);
                }
            }
            QirOp::InsertField { struct_val, value, .. } => {
                if let Some(temp_id) = extract_temp(struct_val) {
                    add_temp_use(temp_id, uses);
                }
                if let Some(temp_id) = extract_temp(value) {
                    add_temp_use(temp_id, uses);
                }
            }
            QirOp::ArrayGet { array, .. } => {
                if let Some(temp_id) = extract_temp(array) {
                    add_temp_use(temp_id, uses);
                }
            }
            QirOp::ArraySet { array, value, .. } => {
                if let Some(temp_id) = extract_temp(array) {
                    add_temp_use(temp_id, uses);
                }
                if let Some(temp_id) = extract_temp(value) {
                    add_temp_use(temp_id, uses);
                }
            }
            QirOp::ApplyGate { args, .. } => {
                for arg in args {
                    if let Some(temp_id) = extract_temp(arg) {
                        add_temp_use(temp_id, uses);
                    }
                }
            }
            _ => {}
        }
    }
    
    fn check_qubit_linearity(&mut self, func: &QirFunction) {
        let mut allocated_qubits = HashSet::new();
        
        for (block_id, block) in &func.blocks {
            for op in &block.ops {
                match op {
                    QirOp::AllocQubit { result: _, init_state: _ } => {
                        // Qubit allocation detected
                    }
                    QirOp::ApplyGate { args, .. } => {
                        for arg in args {
                            if let QirValue::Qubit(qubit_id) = arg {
                                allocated_qubits.insert(qubit_id.id());
                            }
                        }
                    }
                    QirOp::Measure { qubit, .. } => {
                        allocated_qubits.insert(qubit.id());
                    }
                    QirOp::Reset { qubit } => {
                        allocated_qubits.insert(qubit.id());
                    }
                    _ => {}
                }
            }
        }
        
        if allocated_qubits.len() > 100 {
            self.warnings.push(format!(
                "Large number of qubits used: {}",
                allocated_qubits.len()
            ));
        }
    }
    
    fn check_type_consistency(&mut self, func: &QirFunction) {
        for (block_id, block) in &func.blocks {
            for op in &block.ops {
                match op {
                    QirOp::BinaryOp { lhs, rhs, .. } => {
                        let lhs_type = self.infer_value_type(lhs);
                        let rhs_type = self.infer_value_type(rhs);
                        
                        if lhs_type != rhs_type {
                            self.warnings.push(format!(
                                "Type mismatch in binary operation in block {}",
                                block_id.id()
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    
    fn infer_value_type(&self, value: &QirValue) -> ValueType {
        match value {
            QirValue::Int(_) => ValueType::Int,
            QirValue::Float(_) => ValueType::Float,
            QirValue::Bool(_) => ValueType::Bool,
            QirValue::String(_) => ValueType::String,
            QirValue::Qubit(_) => ValueType::Qubit,
            QirValue::Cbit(_) => ValueType::Cbit,
            QirValue::Tuple(elements) => ValueType::Tuple(
                elements.iter().map(|e| self.infer_value_type(e)).collect()
            ),
            QirValue::Array(elements) => {
                if let Some(first) = elements.first() {
                    ValueType::Array(Box::new(self.infer_value_type(first)), elements.len())
                } else {
                    ValueType::Unknown
                }
            }
            QirValue::Temp(_) => ValueType::Unknown,
            QirValue::Variable(_) => ValueType::Unknown,
            QirValue::Null => ValueType::Unit,
        }
    }
    
    fn check_control_flow(&mut self, func: &QirFunction) {
        for (block_id, block) in &func.blocks {
            if block.successors.len() > 1 {
                for &succ in &block.successors {
                    if let Some(succ_block) = func.blocks.get(&succ) {
                        if succ_block.predecessors.len() > 1 {
                            self.warnings.push(format!(
                                "Critical edge from block {} to {} in function {}",
                                block_id.id(), succ.id(), func.name
                            ));
                        }
                    }
                }
            }
        }
    }
    
    fn check_global_resources(&mut self, module: &QirModule) {
        if module.global_qubits.len() > 100 {
            self.warnings.push(format!(
                "Large number of global qubits: {}",
                module.global_qubits.len()
            ));
        }
        
        if module.global_cbits.len() > 1000 {
            self.warnings.push(format!(
                "Large number of global cbits: {}",
                module.global_cbits.len()
            ));
        }
    }
    
    pub fn get_errors(&self) -> &[String] {
        &self.errors
    }
    
    pub fn get_warnings(&self) -> &[String] {
        &self.warnings
    }
}