// ast.rs - COMPLETE FOR PHASE 1.3
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Int,
    Float,
    Bool,
    String,
    Qubit,
    Qreg(usize),
    Cbit,
    Array(Box<Type>, usize),
    Function(Vec<Type>, Box<Type>),
    Unit,
    Tuple(Vec<Type>),
    Named(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Gate {
    H,
    X,
    Y,
    Z,
    CNOT,
    RX(Box<Expr>),
    RY(Box<Expr>),
    RZ(Box<Expr>),
    T,
    S,
    SWAP,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BitString {
    pub bits: Vec<u8>,
    pub span: Span,
}

impl BitString {
    pub fn new(bits: Vec<u8>, span: Span) -> Self {
        BitString { bits, span }
    }
    
    pub fn to_string(&self) -> String {
        let mut s = String::from("|");
        for bit in &self.bits {
            s.push(if *bit == 0 { '0' } else { '1' });
        }
        s.push('>');
        s
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub line: usize,
    pub column: usize,
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(line: usize, column: usize, start: usize, end: usize) -> Self {
        Self { line, column, start, end }
    }
    
    pub fn merge(&self, other: &Span) -> Span {
        Span {
            line: self.line,
            column: self.column,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

impl Default for Span {
    fn default() -> Self {
        Span::new(1, 1, 0, 0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructField {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<StructField>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeAlias {
    pub name: String,
    pub target: Type,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    LiteralInt(i64, Span),
    LiteralFloat(f64, Span),
    LiteralBool(bool, Span),
    LiteralString(String, Span),
    LiteralQubit(BitString, Span),
    
    Variable(String, Span),
    BinaryOp(Box<Expr>, BinaryOp, Box<Expr>, Span),
    UnaryOp(UnaryOp, Box<Expr>, Span),
    Call(String, Vec<Expr>, Span),
    Index(Box<Expr>, Box<Expr>, Span),
    MemberAccess(Box<Expr>, String, Span),
    
    Measure(Box<Expr>, Span),
    GateApply(Box<Gate>, Vec<Expr>, Span),
    
    Tuple(Vec<Expr>, Span),
    StructLiteral(String, Vec<(String, Expr)>, Span),
}

// ADD Hash and Eq derives to BinaryOp
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add, Sub, Mul, Div,
    Eq, Neq, Lt, Gt, Le, Ge,
    And, Or, Xor,
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnaryOp {
    Neg, Not,
    PreIncrement, PostIncrement,
    PreDecrement, PostDecrement,
    Increment, Decrement,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Expr(Expr, Span),
    Let(String, Type, Expr, bool, Span),
    Assign(String, Expr, Span),
    Block(Vec<Stmt>, Span),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>, Span),
    While(Expr, Box<Stmt>, Span),
    ForRange(String, Box<Expr>, Box<Expr>, Option<Box<Expr>>, Box<Stmt>, Span),
    Return(Option<Expr>, Span),
    
    Break(Span),
    Continue(Span),
    
    QIf(Box<Expr>, Box<Stmt>, Option<Box<Stmt>>, Span),
    QForRange(String, Box<Expr>, Box<Expr>, Option<Box<Expr>>, Box<Stmt>, Span),
    
    TypeAlias(TypeAlias, Span),
    StructDef(StructDef, Span),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub mutable: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Type,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub functions: Vec<Function>,
    pub type_aliases: Vec<TypeAlias>,
    pub struct_defs: Vec<StructDef>,
    pub source: Option<String>,
}

impl Expr {
    pub fn span(&self) -> &Span {
        match self {
            Expr::LiteralInt(_, span)
            | Expr::LiteralFloat(_, span)
            | Expr::LiteralBool(_, span)
            | Expr::LiteralString(_, span)
            | Expr::LiteralQubit(_, span)
            | Expr::Variable(_, span)
            | Expr::BinaryOp(_, _, _, span)
            | Expr::UnaryOp(_, _, span)
            | Expr::Call(_, _, span)
            | Expr::Index(_, _, span)
            | Expr::MemberAccess(_, _, span)
            | Expr::Measure(_, span)
            | Expr::GateApply(_, _, span)
            | Expr::Tuple(_, span)
            | Expr::StructLiteral(_, _, span) => span,
        }
    }
}

impl Stmt {
    pub fn span(&self) -> &Span {
        match self {
            Stmt::Expr(_, span)
            | Stmt::Let(_, _, _, _, span)
            | Stmt::Assign(_, _, span)
            | Stmt::Block(_, span)
            | Stmt::If(_, _, _, span)
            | Stmt::While(_, _, span)
            | Stmt::ForRange(_, _, _, _, _, span)
            | Stmt::Return(_, span)
            | Stmt::Break(span)
            | Stmt::Continue(span)
            | Stmt::QIf(_, _, _, span)
            | Stmt::QForRange(_, _, _, _, _, span)
            | Stmt::TypeAlias(_, span)
            | Stmt::StructDef(_, span) => span,
        }
    }
}

impl Gate {
    pub fn arity(&self) -> usize {
        match self {
            Gate::H | Gate::X | Gate::Y | Gate::Z | Gate::RX(_) | 
            Gate::RY(_) | Gate::RZ(_) | Gate::T | Gate::S => 1,
            Gate::CNOT | Gate::SWAP => 2,
        }
    }
}