// src/qir/types.rs - COMPLETE FIXED VERSION
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QubitId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CbitId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TempId(pub usize);

#[derive(Debug, Clone, PartialEq)]
pub enum QirValue {
    Qubit(QubitId),
    Cbit(CbitId),
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Tuple(Vec<QirValue>),
    Array(Vec<QirValue>),
    Temp(TempId),
    Variable(String),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BitState {
    Zero,
    One,
    Plus,
    Minus,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QirParam {
    pub name: String,
    pub ty: QirType,
    pub mutable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum QirType {
    Int,
    Float,
    Bool,
    String,
    Qubit,
    Cbit,
    Qreg(usize),
    Unit,
    Tuple(Vec<QirType>),
    Array(Box<QirType>, usize),
    Struct(String, Vec<QirType>),
    Function(Vec<QirType>, Box<QirType>),
    Pointer(Box<QirType>),
}

impl QubitId {
    pub fn new(id: usize) -> Self {
        Self(id)
    }
    
    pub fn id(&self) -> usize {
        self.0
    }
}

impl CbitId {
    pub fn new(id: usize) -> Self {
        Self(id)
    }
    
    pub fn id(&self) -> usize {
        self.0
    }
}

impl BlockId {
    pub fn new(id: usize) -> Self {
        Self(id)
    }
    
    pub fn id(&self) -> usize {
        self.0
    }
}

impl TempId {
    pub fn new(id: usize) -> Self {
        Self(id)
    }
    
    pub fn id(&self) -> usize {
        self.0
    }
}

impl fmt::Display for QubitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "q{}", self.0)
    }
}

impl fmt::Display for CbitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "c{}", self.0)
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "b{}", self.0)
    }
}

impl fmt::Display for TempId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t{}", self.0)
    }
}

impl QirType {
    pub fn is_quantum(&self) -> bool {
        match self {
            QirType::Qubit | QirType::Qreg(_) => true,
            QirType::Tuple(types) => types.iter().any(|t| t.is_quantum()),
            QirType::Array(elem_type, _) => elem_type.is_quantum(),
            QirType::Struct(_, field_types) => field_types.iter().any(|t| t.is_quantum()),
            _ => false,
        }
    }
    
    pub fn is_classical(&self) -> bool {
        match self {
            QirType::Cbit | QirType::Int | QirType::Float | 
            QirType::Bool | QirType::String => true,
            QirType::Tuple(types) => types.iter().all(|t| t.is_classical()),
            QirType::Array(elem_type, _) => elem_type.is_classical(),
            QirType::Struct(_, field_types) => field_types.iter().all(|t| t.is_classical()),
            _ => false,
        }
    }
    
    pub fn size(&self) -> usize {
        match self {
            QirType::Qubit => 1,
            QirType::Qreg(size) => *size,
            QirType::Cbit => 1,
            QirType::Int => 8,
            QirType::Float => 8,
            QirType::Bool => 1,
            QirType::String => 16,
            QirType::Unit => 0,
            QirType::Tuple(types) => types.iter().map(|t| t.size()).sum(),
            QirType::Array(elem_type, count) => elem_type.size() * count,
            QirType::Struct(_, field_types) => field_types.iter().map(|t| t.size()).sum(),
            QirType::Function(_, _) => 8,
            QirType::Pointer(_) => 8,
        }
    }
}