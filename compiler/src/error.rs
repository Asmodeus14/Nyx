// error.rs - COMPLETE
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CompilerError {
    #[error("Lexer error at position {pos}: {msg}")]
    LexerError { pos: usize, msg: String },
    
    #[error("Parser error: expected {expected}, found {found}")]
    ParseError { expected: String, found: String },
    
    #[error("Type error: {0}")]
    TypeError(String),
    
    #[error("Quantum resource error: {0}")]
    QuantumError(String),
    
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}