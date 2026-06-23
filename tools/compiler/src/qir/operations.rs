// src/qir/operations.rs - COMPLETE FIXED VERSION
use crate::ast::{BinaryOp as AstBinaryOp, UnaryOp as AstUnaryOp, Gate as AstGate};
use super::types::{QubitId, CbitId, TempId, BlockId, QirValue, BitState};

#[derive(Debug, Clone, PartialEq)]
pub enum QirGate {
    // Standard gates
    H,
    X,
    Y,
    Z,
    CNOT,
    SWAP,
    
    // Phase gates
    T,
    Tdg,
    S,
    Sdg,
    
    // Rotation gates
    RX(f64),
    RY(f64),
    RZ(f64),
    U3(f64, f64, f64),
    
    // Multi-qubit gates
    Toffoli,
    Fredkin,
    
    // Custom gates
    Custom { name: String, matrix: Vec<Vec<f64>> },
}

impl QirGate {
    pub fn arity(&self) -> usize {
        match self {
            QirGate::H | QirGate::X | QirGate::Y | QirGate::Z |
            QirGate::T | QirGate::Tdg | QirGate::S | QirGate::Sdg |
            QirGate::RX(_) | QirGate::RY(_) | QirGate::RZ(_) | QirGate::U3(_, _, _) => 1,
            QirGate::CNOT | QirGate::SWAP => 2,
            QirGate::Toffoli | QirGate::Fredkin => 3,
            QirGate::Custom { matrix, .. } => {
                let size = matrix.len();
                (size as f64).log2().round() as usize
            }
        }
    }
    
    pub fn is_clifford(&self) -> bool {
        matches!(
            self,
            QirGate::H | QirGate::X | QirGate::Y | QirGate::Z |
            QirGate::CNOT | QirGate::S | QirGate::Sdg
        )
    }
    
    pub fn is_universal(&self) -> bool {
        matches!(self, QirGate::U3(_, _, _))
    }
    
    pub fn from_ast_gate(gate: &AstGate) -> Option<Self> {
        match gate {
            AstGate::H => Some(QirGate::H),
            AstGate::X => Some(QirGate::X),
            AstGate::Y => Some(QirGate::Y),
            AstGate::Z => Some(QirGate::Z),
            AstGate::CNOT => Some(QirGate::CNOT),
            AstGate::RX(expr) => {
                // For now, use placeholder
                Some(QirGate::RX(0.0))
            }
            AstGate::RY(expr) => Some(QirGate::RY(0.0)),
            AstGate::RZ(expr) => Some(QirGate::RZ(0.0)),
            AstGate::T => Some(QirGate::T),
            AstGate::S => Some(QirGate::S),
            AstGate::SWAP => Some(QirGate::SWAP),
            _ => None,
        }
    }
    
    pub fn to_qasm_name(&self) -> String {
        match self {
            QirGate::H => "h".to_string(),
            QirGate::X => "x".to_string(),
            QirGate::Y => "y".to_string(),
            QirGate::Z => "z".to_string(),
            QirGate::CNOT => "cx".to_string(),
            QirGate::SWAP => "swap".to_string(),
            QirGate::T => "t".to_string(),
            QirGate::S => "s".to_string(),
            QirGate::RX(angle) => format!("rx({})", angle),
            QirGate::RY(angle) => format!("ry({})", angle),
            QirGate::RZ(angle) => format!("rz({})", angle),
            QirGate::U3(theta, phi, lambda) => format!("u3({}, {}, {})", theta, phi, lambda),
            QirGate::Toffoli => "ccx".to_string(),
            _ => format!("// {:?}", self),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum QirOp {
    // Quantum operations
    AllocQubit { result: TempId, init_state: Option<BitState> },
    ApplyGate { gate: QirGate, args: Vec<QirValue>, result: Option<TempId> },
    Measure { qubit: QubitId, cbit: CbitId },
    Reset { qubit: QubitId },
    
    // Classical operations
    AllocCbit { result: TempId, init_value: Option<u8> },
    ClassicalAssign { target: TempId, value: QirValue },
    BinaryOp { op: AstBinaryOp, lhs: QirValue, rhs: QirValue, result: TempId },
    UnaryOp { op: AstUnaryOp, operand: QirValue, result: TempId },
    
    // Control flow
    Jump { target: BlockId },
    Branch { cond: QirValue, then_block: BlockId, else_block: BlockId },
    Return { value: Option<QirValue> },
    
    // Memory operations
    Load { ptr: TempId, result: TempId },
    Store { ptr: TempId, value: QirValue },
    GetElementPtr { base: TempId, indices: Vec<usize>, result: TempId },
    
    // Struct operations
    MakeStruct { field_values: Vec<QirValue>, result: TempId },
    ExtractField { struct_val: QirValue, field_index: usize, result: TempId },
    InsertField { struct_val: QirValue, field_index: usize, value: QirValue, result: TempId },
    
    // Array operations
    MakeArray { elements: Vec<QirValue>, result: TempId },
    ArrayGet { array: QirValue, index: usize, result: TempId },
    ArraySet { array: QirValue, index: usize, value: QirValue, result: TempId },
    
    // Special operations
    Phi { incoming: Vec<(BlockId, QirValue)>, result: TempId },
    Comment(String),
}