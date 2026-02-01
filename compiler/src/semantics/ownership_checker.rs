// src/semantics/ownership_checker.rs - FIXED
use crate::ast::*;
use crate::semantics::symbols::TypeRegistry;
use crate::semantics::errors::SemanticError;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct OwnershipChecker {
    source: String,
    qubit_states: HashMap<String, QubitState>,
    cbit_states: HashMap<String, CbitState>,
    current_scope: Vec<HashMap<String, (Type, bool)>>,
    errors: Vec<SemanticError>,
    type_registry: TypeRegistry,
    struct_defs: HashMap<String, StructDef>,
    used_qubits: HashSet<String>,
    measured_qubits: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum QubitState {
    Available,
    Measured,
    Transformed,
    Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CbitState {
    Uninitialized,
    Measured,
    Used,
}

impl OwnershipChecker {
    pub fn new(source: &str) -> Self {
        Self {
            source: source.to_string(),
            qubit_states: HashMap::new(),
            cbit_states: HashMap::new(),
            current_scope: vec![HashMap::new()],
            errors: Vec::new(),
            type_registry: TypeRegistry::new(),
            struct_defs: HashMap::new(),
            used_qubits: HashSet::new(),
            measured_qubits: HashSet::new(),
        }
    }
    
    pub fn set_type_registry(&mut self, registry: TypeRegistry) {
        self.type_registry = registry;
    }
    
    pub fn check_program(&mut self, program: &Program) -> Result<(), Vec<SemanticError>> {
        // Collect type definitions first from Program
        for type_alias in &program.type_aliases {
            self.type_registry.add_type_alias(type_alias.name.clone(), type_alias.target.clone());
        }
        
        for struct_def in &program.struct_defs {
            self.type_registry.add_struct_def(struct_def.clone());
            self.struct_defs.insert(struct_def.name.clone(), struct_def.clone());
        }
        
        // Check each function
        for function in &program.functions {
            self.check_function(function);
        }
        
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(self.errors.clone())
        }
    }
    
    fn check_function(&mut self, function: &Function) {
        self.current_scope.push(HashMap::new());
        
        // Register parameters
        for param in &function.params {
            let resolved_ty = match self.type_registry.resolve_type(&param.ty) {
                Ok(ty) => ty,
                Err(e) => {
                    self.errors.push(SemanticError::new(
                        &param.span,
                        &format!("Invalid parameter type: {}", e),
                        Some("Parameter type must be valid"),
                    ));
                    continue;
                }
            };
            
            self.current_scope.last_mut().unwrap().insert(
                param.name.clone(),
                (resolved_ty.clone(), param.mutable)
            );
        }
        
        // Check function body
        for stmt in &function.body {
            self.check_statement(stmt);
        }
        
        self.current_scope.pop();
    }
    
    fn check_statement(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let(name, ty, expr, mutable, span) => {
                let resolved_ty = match self.type_registry.resolve_type(ty) {
                    Ok(ty) => ty,
                    Err(e) => {
                        self.errors.push(SemanticError::new(
                            span,
                            &format!("Invalid type in let statement: {}", e),
                            Some("Type must be valid"),
                        ));
                        return;
                    }
                };
                
                self.current_scope.last_mut().unwrap().insert(
                    name.clone(),
                    (resolved_ty.clone(), *mutable)
                );
                
                // Check quantum type mutability
                if *mutable {
                    if let Ok(true) = self.type_registry.is_quantum_type(&resolved_ty) {
                        self.errors.push(SemanticError::new(
                            span,
                            "Quantum types cannot be mutable",
                            Some("Remove 'mut' keyword from quantum variable"),
                        ));
                    }
                }
                
                // Check expression
                self.check_expression(expr);
            }
            
            Stmt::Assign(name, expr, span) => {
                // Check variable exists
                let (ty, mutable) = match self.lookup_variable(name) {
                    Some(info) => info,
                    None => {
                        self.errors.push(SemanticError::new(
                            span,
                            &format!("Variable '{}' not found", name),
                            Some("Variable must be declared before assignment"),
                        ));
                        return;
                    }
                };
                
                if !mutable {
                    self.errors.push(SemanticError::new(
                        span,
                        &format!("Cannot assign to immutable variable '{}'", name),
                        Some("Declare variable with 'mut' to make it mutable"),
                    ));
                }
                
                // Check quantum type reassignment
                if let Ok(true) = self.type_registry.is_quantum_type(&ty) {
                    self.errors.push(SemanticError::new(
                        span,
                        &format!("Cannot reassign quantum variable '{}'", name),
                        Some("Quantum variables follow affine typing and cannot be reassigned"),
                    ));
                }
                
                // Check expression
                self.check_expression(expr);
            }
            
            Stmt::Expr(expr, span) => {
                self.check_expression(expr);
            }
            
            Stmt::Return(expr, span) => {
                if let Some(expr) = expr {
                    self.check_expression(expr);
                }
            }
            
            Stmt::Block(stmts, _) => {
                self.current_scope.push(HashMap::new());
                for stmt in stmts {
                    self.check_statement(stmt);
                }
                self.current_scope.pop();
            }
            
            _ => {} // Other statements handled by semantic analyzer
        }
    }
    
    fn check_expression(&mut self, expr: &Expr) {
        match expr {
            Expr::Variable(name, span) => {
                if let Some((ty, _)) = self.lookup_variable(name) {
                    // Check quantum resource usage
                    if let Ok(true) = self.type_registry.is_quantum_type(&ty) {
                        if let Some(state) = self.qubit_states.get(name) {
                            match state {
                                QubitState::Measured => {
                                    self.errors.push(SemanticError::new(
                                        span,
                                        &format!("Qubit '{}' used after measurement", name),
                                        Some("Quantum resources are affine and cannot be used after measurement"),
                                    ));
                                }
                                QubitState::Consumed => {
                                    self.errors.push(SemanticError::new(
                                        span,
                                        &format!("Qubit '{}' already consumed", name),
                                        Some("Quantum resources can only be used once"),
                                    ));
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            
            Expr::Measure(qubit_expr, span) => {
                self.check_expression(qubit_expr);
                
                // Mark measured qubits
                if let Expr::Variable(name, _) = &**qubit_expr {
                    self.qubit_states.insert(name.clone(), QubitState::Measured);
                } else if let Expr::MemberAccess(base, field, _) = &**qubit_expr {
                    // Handle struct member measurement
                    if let Expr::Variable(struct_name, _) = &**base {
                        let full_name = format!("{}.{}", struct_name, field);
                        self.qubit_states.insert(full_name, QubitState::Measured);
                    }
                }
            }
            
            Expr::GateApply(_gate, args, span) => {
                for arg in args {
                    self.check_expression(arg);
                    
                    // Check quantum arguments are not measured
                    if let Expr::Variable(name, _) = arg {
                        if let Some(state) = self.qubit_states.get(name) {
                            if *state == QubitState::Measured {
                                self.errors.push(SemanticError::new(
                                    span,
                                    &format!("Qubit '{}' used in gate after measurement", name),
                                    Some("Quantum resources cannot be used in gates after measurement"),
                                ));
                            }
                        }
                    } else if let Expr::MemberAccess(base, field, _) = arg {
                        // Check struct member
                        if let Expr::Variable(struct_name, _) = &**base {
                            let full_name = format!("{}.{}", struct_name, field);
                            if let Some(state) = self.qubit_states.get(&full_name) {
                                if *state == QubitState::Measured {
                                    self.errors.push(SemanticError::new(
                                        span,
                                        &format!("Struct member '{}.{}' used in gate after measurement", struct_name, field),
                                        Some("Quantum resources cannot be used in gates after measurement"),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            
            Expr::BinaryOp(left, _, right, _) => {
                self.check_expression(left);
                self.check_expression(right);
            }
            
            Expr::UnaryOp(_, operand, _) => {
                self.check_expression(operand);
            }
            
            Expr::Call(_, args, _) => {
                for arg in args {
                    self.check_expression(arg);
                }
            }
            
            _ => {} // Other expressions don't need special checking
        }
    }
    
    fn lookup_variable(&self, name: &str) -> Option<(Type, bool)> {
        for scope in self.current_scope.iter().rev() {
            if let Some((ty, mutable)) = scope.get(name) {
                return Some((ty.clone(), *mutable));
            }
        }
        None
    }
    
    pub fn format_error(&self, error: &SemanticError) -> String {
        error.format_with_source(&self.source)
    }
}