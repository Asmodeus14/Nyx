// semantics/mod.rs - COMPLETE FOR PHASE 1.4
pub mod symbols;
pub mod analyzer;
pub mod errors;
pub mod ownership_checker;

pub use analyzer::SemanticAnalyzer;
pub use errors::SemanticError;
pub use ownership_checker::OwnershipChecker;
pub use symbols::{TypeRegistry, SymbolTable};