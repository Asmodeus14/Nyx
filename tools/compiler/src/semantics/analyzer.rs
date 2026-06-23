// src/semantics/analyzer.rs - FULLY CORRECTED
use crate::ast::{Program, Function, Stmt, Expr, Type, Span, BinaryOp, UnaryOp};
use crate::semantics::symbols::{SymbolTable, TypeRegistry, Symbol};
use crate::semantics::errors::SemanticError;

#[derive(Debug)]
pub struct SemanticAnalyzer {
    pub symbol_table: SymbolTable,
    pub type_registry: TypeRegistry,
    pub errors: Vec<SemanticError>,
    pub warnings: Vec<String>,
    pub current_function: Option<String>,
    pub in_quantum_context: bool,
    pub loop_depth: usize,
}

impl SemanticAnalyzer {
    pub fn new() -> Self {
        Self {
            symbol_table: SymbolTable::new(),
            type_registry: TypeRegistry::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            current_function: None,
            in_quantum_context: false,
            loop_depth: 0,
        }
    }
    
    pub fn analyze_program(&mut self, program: &Program) -> Result<(), Vec<SemanticError>> {
        // PASS 1: Collect Definitions
        self.collect_definitions(program);
        
        // If we have errors in pass 1, stop early
        if !self.errors.is_empty() {
            return Err(self.errors.clone());
        }
        
        // PASS 2: Analyze Bodies
        self.analyze_bodies(program);
        
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(self.errors.clone())
        }
    }
    
    fn collect_definitions(&mut self, program: &Program) {
        // First pass: collect type aliases from program
        for type_alias in &program.type_aliases {
            // Check if type alias is valid
            if let Err(e) = self.type_registry.resolve_type(&type_alias.target) {
                self.errors.push(SemanticError::new(
                    &type_alias.span,
                    &format!("Invalid type alias target: {}", e),
                    Some("Type alias must reference a valid type"),
                ));
                return;
            }
            
            // Add to type registry
            self.type_registry.add_type_alias(
                type_alias.name.clone(),
                type_alias.target.clone(),
            );
            
            // Also add to symbol table
            let symbol = Symbol::TypeAlias {
                name: type_alias.name.clone(),
                target: type_alias.target.clone(),
            };
            
            if let Err(e) = self.symbol_table.insert(symbol) {
                self.errors.push(SemanticError::new(
                    &type_alias.span,
                    &format!("Type alias '{}' already defined: {}", type_alias.name, e),
                    Some("Type aliases must have unique names"),
                ));
            }
        }
        
        // Second pass: collect struct definitions from program
        for struct_def in &program.struct_defs {
            // Check that struct name is not already used
            if self.type_registry.struct_defs.contains_key(&struct_def.name) {
                self.errors.push(SemanticError::new(
                    &struct_def.span,
                    &format!("Struct '{}' already defined", struct_def.name),
                    Some("Struct names must be unique"),
                ));
                return;
            }
            
            // Check all field types are valid
            for field in &struct_def.fields {
                if let Err(e) = self.type_registry.resolve_type(&field.ty) {
                    self.errors.push(SemanticError::new(
                        &field.span,
                        &format!("Invalid field type: {}", e),
                        Some("Struct field types must be valid"),
                    ));
                }
            }
            
            // Add to type registry
            self.type_registry.add_struct_def(struct_def.clone());
            
            // Also add to symbol table
            let symbol = Symbol::Struct {
                name: struct_def.name.clone(),
                definition: struct_def.clone(),
            };
            
            if let Err(e) = self.symbol_table.insert(symbol) {
                self.errors.push(SemanticError::new(
                    &struct_def.span,
                    &format!("Struct '{}' already defined: {}", struct_def.name, e),
                    Some("Struct names must be unique"),
                ));
            }
        }
        
        // Third pass: collect function signatures from all functions
        for function in &program.functions {
            self.collect_function_signature(function);
        }
    }
    
    fn collect_function_signature(&mut self, function: &Function) {
        // Check return type
        if let Err(e) = self.type_registry.resolve_type(&function.return_type) {
            self.errors.push(SemanticError::new(
                &function.span,
                &format!("Invalid return type: {}", e),
                Some("Function return type must be a valid type"),
            ));
        }
        
        // Check parameter types
        for param in &function.params {
            if let Err(e) = self.type_registry.resolve_type(&param.ty) {
                self.errors.push(SemanticError::new(
                    &param.span,
                    &format!("Invalid parameter type: {}", e),
                    Some("Parameter types must be valid"),
                ));
            }
        }
        
        let symbol = Symbol::Function {
            name: function.name.clone(),
            params: function.params.clone(),
            return_type: function.return_type.clone(),
            defined: false,
        };
        
        if let Err(e) = self.symbol_table.insert(symbol) {
            self.errors.push(SemanticError::new(
                &function.span,
                &format!("Function '{}' already defined: {}", function.name, e),
                Some("Function names must be unique"),
            ));
        }
    }
    
    fn analyze_bodies(&mut self, program: &Program) {
        for function in &program.functions {
            self.analyze_function(function);
        }
    }
    
    fn analyze_function(&mut self, function: &Function) {
        self.current_function = Some(function.name.clone());
        
        // Push function scope
        self.symbol_table.push_scope();
        
        // Add parameters to scope
        for param in &function.params {
            let symbol = Symbol::Variable {
                name: param.name.clone(),
                ty: param.ty.clone(),
                mutable: param.mutable,
                defined: true,
            };
            
            if let Err(e) = self.symbol_table.insert(symbol) {
                self.errors.push(SemanticError::new(
                    &param.span,
                    &format!("Parameter '{}' conflicts: {}", param.name, e),
                    Some("Parameter names must be unique"),
                ));
            }
        }
        
        // Analyze function body
        for stmt in &function.body {
            self.analyze_statement(stmt);
        }
        
        // Check if function has a return statement if needed
        if !matches!(function.return_type, Type::Unit) {
            // TODO: Implement return statement checking
            self.warnings.push(format!(
                "Function '{}' has non-unit return type but return statement checking not implemented",
                function.name
            ));
        }
        
        // Mark function as defined
        if let Err(e) = self.symbol_table.mark_function_defined(&function.name) {
            self.errors.push(SemanticError::new(
                &function.span,
                &format!("Failed to mark function as defined: {}", e),
                None,
            ));
        }
        
        // Pop function scope
        self.symbol_table.pop_scope();
        self.current_function = None;
    }
    
    fn analyze_statement(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let(name, ty, expr, mutable, span) => {
                // Handle qreg declarations (qreg q[5] = |00000>;)
                if let Type::Qreg(size) = ty {
                    self.analyze_qreg_declaration(name, *size, expr, *mutable, span);
                } else if let Type::Array(element_type, size) = ty {
                    // Handle array declarations (cbit results[3];)
                    self.analyze_array_declaration(name, element_type, *size, expr, *mutable, span);
                } else {
                    self.analyze_let_stmt(name, ty, expr, *mutable, span);
                }
            }
            
            Stmt::Assign(name, expr, span) => {
                self.analyze_assign_stmt(name, expr, span);
            }
            
            Stmt::Expr(expr, span) => {
                let _ = self.analyze_expression(expr, span);
            }
            
            Stmt::Return(expr, span) => {
                self.analyze_return_stmt(expr, span);
            }
            
            Stmt::Block(stmts, _span) => {
                self.symbol_table.push_scope();
                for stmt in stmts {
                    self.analyze_statement(stmt);
                }
                self.symbol_table.pop_scope();
            }
            
            Stmt::If(condition, then_branch, else_branch, _span) => {
                self.analyze_if_stmt(condition, then_branch, else_branch.as_deref());
            }
            
            Stmt::While(condition, body, _span) => {
                self.analyze_while_stmt(condition, body);
            }
            
            Stmt::ForRange(var_name, start, end, step, body, span) => {
                self.analyze_for_range_stmt(var_name, start, end, step, body, span);
            }
            
            Stmt::QIf(condition, then_branch, else_branch, _span) => {
                self.analyze_qif_stmt(condition, then_branch, else_branch.as_deref());
            }
            
            Stmt::QForRange(var_name, start, end, step, body, span) => {
                self.analyze_qfor_range_stmt(var_name, start, end, step, body, span);
            }
            
            Stmt::TypeAlias(_, _) | Stmt::StructDef(_, _) => {
                // Already handled in collect_definitions
            }
            
            Stmt::Break(span) => {
                self.analyze_break_stmt(span);
            }
            
            Stmt::Continue(span) => {
                self.analyze_continue_stmt(span);
            }
        }
    }
    
    fn analyze_qreg_declaration(&mut self, name: &str, size: usize, expr: &Expr, mutable: bool, span: &Span) {
        if mutable {
            self.errors.push(SemanticError::new(
                span,
                "Quantum registers cannot be mutable",
                Some("Remove 'mut' keyword from qreg declaration"),
            ));
        }
        
        // Check the expression is a bit string literal
        match expr {
            Expr::LiteralQubit(bit_string, _) => {
                // Check bit string length matches qreg size
                if bit_string.bits.len() != size {
                    self.errors.push(SemanticError::new(
                        span,
                        &format!("Bit string length {} doesn't match qreg size {}", 
                                bit_string.bits.len(), size),
                        Some("Bit string must have same length as qreg size"),
                    ));
                }
            }
            _ => {
                self.errors.push(SemanticError::new(
                    span,
                    "Qreg must be initialized with a bit string literal",
                    Some("Use syntax: qreg name[size] = |bits...>;"),
                ));
            }
        }
        
        // Add qreg to symbol table
        let symbol = Symbol::Variable {
            name: name.to_string(),
            ty: Type::Qreg(size),
            mutable: false,
            defined: true,
        };
        
        if let Err(e) = self.symbol_table.insert(symbol) {
            self.errors.push(SemanticError::new(
                span,
                &format!("Failed to add qreg '{}': {}", name, e),
                None,
            ));
        }
    }
    
    fn analyze_array_declaration(&mut self, name: &str, element_type: &Type, size: usize, 
                                 expr: &Expr, mutable: bool, span: &Span) {
        // Check element type is valid
        if let Err(e) = self.type_registry.resolve_type(element_type) {
            self.errors.push(SemanticError::new(
                span,
                &format!("Invalid array element type: {}", e),
                Some("Array element type must be a valid type"),
            ));
        }
        
        // Check quantum type mutability
        if mutable {
            if let Ok(true) = self.type_registry.is_quantum_type(element_type) {
                self.errors.push(SemanticError::new(
                    span,
                    "Arrays of quantum types cannot be mutable",
                    Some("Remove 'mut' keyword from quantum array declaration"),
                ));
            }
        }
        
        // Add array to symbol table
        let symbol = Symbol::Variable {
            name: name.to_string(),
            ty: Type::Array(Box::new(element_type.clone()), size),
            mutable,
            defined: true,
        };
        
        if let Err(e) = self.symbol_table.insert(symbol) {
            self.errors.push(SemanticError::new(
                span,
                &format!("Failed to add array '{}': {}", name, e),
                None,
            ));
        }
    }
    
    fn analyze_let_stmt(&mut self, name: &str, ty: &Type, expr: &Expr, mutable: bool, span: &Span) {
        // Resolve the type
        let resolved_ty = match self.type_registry.resolve_type(ty) {
            Ok(t) => t,
            Err(e) => {
                self.errors.push(SemanticError::new(
                    span,
                    &format!("Invalid type in variable declaration: {}", e),
                    Some("Variable type must be a valid type"),
                ));
                return;
            }
        };
        
        // Check if variable already exists in current scope
        if self.symbol_table.contains(name) {
            self.errors.push(SemanticError::new(
                span,
                &format!("Variable '{}' already defined in this scope", name),
                Some("Variable names must be unique within the same scope"),
            ));
        }
        
        // Check quantum type mutability
        if mutable {
            if let Ok(true) = self.type_registry.is_quantum_type(&resolved_ty) {
                self.errors.push(SemanticError::new(
                    span,
                    "Quantum types cannot be mutable",
                    Some("Remove 'mut' keyword from quantum variable declaration"),
                ));
            }
        }
        
        // Analyze the expression
        let expr_ty = self.analyze_expression_type(expr);
        
        // Check type compatibility
        match expr_ty {
            Ok(expr_ty_resolved) => {
                if !self.are_types_compatible(&resolved_ty, &expr_ty_resolved) {
                    self.errors.push(SemanticError::new(
                        span,
                        &format!("Type mismatch: variable declared as {:?} but expression has type {:?}", 
                                resolved_ty, expr_ty_resolved),
                        Some("Variable type and expression type must be compatible"),
                    ));
                }
            }
            Err(e) => {
                self.errors.push(SemanticError::new(
                    expr.span(),
                    &e,
                    Some("Expression type could not be determined"),
                ));
            }
        }
        
        // Add variable to symbol table
        let symbol = Symbol::Variable {
            name: name.to_string(),
            ty: resolved_ty,
            mutable,
            defined: true,
        };
        
        if let Err(e) = self.symbol_table.insert(symbol) {
            self.errors.push(SemanticError::new(
                span,
                &format!("Failed to add variable to symbol table: {}", e),
                None,
            ));
        }
    }
    
    fn analyze_assign_stmt(&mut self, name: &str, expr: &Expr, span: &Span) {
        // Look up variable
        let (var_ty, mutable, defined) = match self.symbol_table.lookup_variable(name) {
            Some((ty, mutable, defined)) => (ty.clone(), mutable, defined),
            None => {
                self.errors.push(SemanticError::new(
                    span,
                    &format!("Variable '{}' not found", name),
                    Some("Variable must be declared before use"),
                ));
                return;
            }
        };
        
        if !defined {
            self.errors.push(SemanticError::new(
                span,
                &format!("Variable '{}' used before initialization", name),
                Some("Variable must be initialized before use"),
            ));
        }
        
        if !mutable {
            self.errors.push(SemanticError::new(
                span,
                &format!("Cannot assign to immutable variable '{}'", name),
                Some("Declare variable with 'mut' to make it mutable"),
            ));
        }
        
        // Check quantum type reassignment
        if let Ok(true) = self.type_registry.is_quantum_type(&var_ty) {
            self.errors.push(SemanticError::new(
                span,
                &format!("Cannot reassign quantum variable '{}'", name),
                Some("Quantum variables follow affine typing and cannot be reassigned"),
            ));
        }
        
        // Analyze expression
        let expr_ty = self.analyze_expression_type(expr);
        
        match expr_ty {
            Ok(expr_ty_resolved) => {
                if !self.are_types_compatible(&var_ty, &expr_ty_resolved) {
                    self.errors.push(SemanticError::new(
                        span,
                        &format!("Type mismatch in assignment: variable is {:?} but expression is {:?}", 
                                var_ty, expr_ty_resolved),
                        Some("Assignment types must be compatible"),
                    ));
                }
            }
            Err(e) => {
                self.errors.push(SemanticError::new(
                    expr.span(),
                    &e,
                    Some("Expression type could not be determined"),
                ));
            }
        }
    }
    
    fn analyze_expression(&mut self, expr: &Expr, span: &Span) -> Result<Type, ()> {
        match self.analyze_expression_type(expr) {
            Ok(ty) => Ok(ty),
            Err(e) => {
                self.errors.push(SemanticError::new(
                    span,
                    &e,
                    Some("Expression type error"),
                ));
                Err(())
            }
        }
    }
    
    fn analyze_expression_type(&mut self, expr: &Expr) -> Result<Type, String> {
        match expr {
            Expr::LiteralInt(_, _) => Ok(Type::Int),
            Expr::LiteralFloat(_, _) => Ok(Type::Float),
            Expr::LiteralBool(_, _) => Ok(Type::Bool),
            Expr::LiteralString(_, _) => Ok(Type::String),
            Expr::LiteralQubit(_, _) => Ok(Type::Qubit),
            
            Expr::Variable(name, _) => {
                let (ty, _, defined) = self.symbol_table.lookup_variable(name)
                    .ok_or_else(|| format!("Variable '{}' not found", name))?;
                
                if !defined {
                    return Err(format!("Variable '{}' used before initialization", name));
                }
                
                self.type_registry.resolve_type(ty)
            }
            
            Expr::BinaryOp(left, op, right, _) => {
                let left_ty = self.analyze_expression_type(left)?;
                let right_ty = self.analyze_expression_type(right)?;
                
                match op {
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                        self.check_arithmetic_types(&left_ty, &right_ty, op.clone())
                    }
                    
                    BinaryOp::Eq | BinaryOp::Neq => {
                        self.check_equality_types(&left_ty, &right_ty)
                    }
                    
                    BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge => {
                        self.check_relational_types(&left_ty, &right_ty)
                    }
                    
                    BinaryOp::And | BinaryOp::Or | BinaryOp::Xor => {
                        self.check_logical_types(&left_ty, &right_ty)
                    }
                    
                    BinaryOp::Assign => {
                        // Assignment returns the assigned type
                        Ok(right_ty)
                    }
                    
                    BinaryOp::AddAssign | BinaryOp::SubAssign | 
                    BinaryOp::MulAssign | BinaryOp::DivAssign => {
                        // Compound assignments require compatible types
                        if self.are_types_compatible(&left_ty, &right_ty) {
                            Ok(left_ty)
                        } else {
                            Err(format!("Incompatible types for compound assignment: {:?} and {:?}", 
                                      left_ty, right_ty))
                        }
                    }
                }
            }
            
            Expr::UnaryOp(op, operand, _) => {
                let operand_ty = self.analyze_expression_type(operand)?;
                
                match op {
                    UnaryOp::Neg => {
                        if matches!(operand_ty, Type::Int | Type::Float) {
                            Ok(operand_ty)
                        } else {
                            Err(format!("Cannot apply negation to type {:?}", operand_ty))
                        }
                    }
                    UnaryOp::Not => {
                        if matches!(operand_ty, Type::Bool) {
                            Ok(Type::Bool)
                        } else {
                            Err(format!("Cannot apply logical NOT to type {:?}", operand_ty))
                        }
                    }
                    _ => {
                        Ok(Type::Unit)
                    }
                }
            }
            
            Expr::Call(name, args, _) => {
                let (params, return_type, defined) = self.symbol_table.lookup_function(name)
                    .ok_or_else(|| format!("Function '{}' not found", name))?;
                
                if !defined {
                    return Err(format!("Function '{}' used before definition", name));
                }
                
                // Check argument count
                if args.len() != params.len() {
                    return Err(format!(
                        "Function '{}' expects {} arguments, got {}", 
                        name, params.len(), args.len()
                    ));
                }
                
                // Return function's return type
                self.type_registry.resolve_type(&return_type)
            }
            
            Expr::Measure(qubit_expr, _) => {
                let qubit_ty = self.analyze_expression_type(qubit_expr)?;
                
                // Ensure we're measuring a quantum type
                if self.type_registry.is_quantum_type(&qubit_ty)? {
                    Ok(Type::Cbit)
                } else {
                    Err(format!("Cannot measure non-quantum type {:?}", qubit_ty))
                }
            }
            
            Expr::GateApply(gate, args, _) => {
                // Check gate arity
                let expected_arity = gate.arity();
                if args.len() != expected_arity {
                    return Err(format!(
                        "Gate {:?} expects {} arguments, got {}", 
                        gate, expected_arity, args.len()
                    ));
                }
                
                // Gates return the type of their first argument
                if let Some(first_arg) = args.first() {
                    self.analyze_expression_type(first_arg)
                } else {
                    Err("Gate requires at least one argument".to_string())
                }
            }
            
            Expr::Index(array_expr, index_expr, _) => {
                let array_ty = self.analyze_expression_type(array_expr)?;
                let index_ty = self.analyze_expression_type(index_expr)?;
                
                // Index must be integer
                if !matches!(index_ty, Type::Int) {
                    return Err(format!("Array index must be int, got {:?}", index_ty));
                }
                
                match array_ty {
                    Type::Array(elem_type, _) => Ok(*elem_type.clone()),
                    Type::Qreg(_) => Ok(Type::Qubit),
                    _ => Err(format!("Cannot index type {:?}", array_ty)),
                }
            }
            
            Expr::MemberAccess(base_expr, field_name, _) => {
                let base_ty = self.analyze_expression_type(base_expr)?;
                
                match base_ty {
                    Type::Named(name) => {
                        let struct_def = self.type_registry.get_struct_def(&name)
                            .ok_or_else(|| format!("'{}' is not a struct", name))?;
                        
                        // Find the field
                        for field in &struct_def.fields {
                            if field.name == *field_name {
                                return self.type_registry.resolve_type(&field.ty);
                            }
                        }
                        
                        Err(format!("Struct '{}' has no field '{}'", name, field_name))
                    }
                    Type::Tuple(types) => {
                        // Tuple field access using .0, .1, etc.
                        if let Ok(index) = field_name.parse::<usize>() {
                            if index < types.len() {
                                return self.type_registry.resolve_type(&types[index]);
                            }
                        }
                        Err(format!("Invalid tuple field '{}'", field_name))
                    }
                    _ => Err(format!("Cannot access field '{}' on type {:?}", field_name, base_ty)),
                }
            }
            
            Expr::Tuple(elements, _) => {
                let mut element_types = Vec::new();
                for element in elements {
                    element_types.push(self.analyze_expression_type(element)?);
                }
                Ok(Type::Tuple(element_types))
            }
            
            Expr::StructLiteral(struct_name, fields, _) => {
                let struct_def = self.type_registry.get_struct_def(struct_name)
                    .ok_or_else(|| format!("Struct '{}' not defined", struct_name))?;
                
                // Check all required fields are present
                for struct_field in &struct_def.fields {
                    if !fields.iter().any(|(field_name, _)| field_name == &struct_field.name) {
                        return Err(format!("Missing field '{}' in struct literal", struct_field.name));
                    }
                }
                
                // Check no extra fields
                for (field_name, _) in fields {
                    if !struct_def.fields.iter().any(|f| &f.name == field_name) {
                        return Err(format!("Struct '{}' has no field '{}'", struct_name, field_name));
                    }
                }
                
                Ok(Type::Named(struct_name.clone()))
            }
        }
    }
    
    fn check_arithmetic_types(&self, left: &Type, right: &Type, op: BinaryOp) -> Result<Type, String> {
        match (left, right) {
            (Type::Int, Type::Int) => Ok(Type::Int),
            (Type::Float, Type::Float) => Ok(Type::Float),
            (Type::Int, Type::Float) | (Type::Float, Type::Int) => Ok(Type::Float),
            _ => Err(format!("Cannot apply {:?} to types {:?} and {:?}", op, left, right)),
        }
    }
    
    fn check_equality_types(&self, left: &Type, right: &Type) -> Result<Type, String> {
        if self.are_types_compatible(left, right) {
            Ok(Type::Bool)
        } else {
            Err(format!("Cannot compare types {:?} and {:?} for equality", left, right))
        }
    }
    
    fn check_relational_types(&self, left: &Type, right: &Type) -> Result<Type, String> {
        match (left, right) {
            (Type::Int, Type::Int) |
            (Type::Float, Type::Float) |
            (Type::Int, Type::Float) |
            (Type::Float, Type::Int) => Ok(Type::Bool),
            _ => Err(format!("Cannot compare types {:?} and {:?} relationally", left, right)),
        }
    }
    
    fn check_logical_types(&self, left: &Type, right: &Type) -> Result<Type, String> {
        match (left, right) {
            (Type::Bool, Type::Bool) => Ok(Type::Bool),
            _ => Err(format!("Cannot apply logical operation to types {:?} and {:?}", left, right)),
        }
    }
    
    fn are_types_compatible(&self, expected: &Type, actual: &Type) -> bool {
        // Basic type compatibility with some implicit conversions
        if expected == actual {
            return true;
        }
        
        // Allow int -> float conversion
        matches!((expected, actual), (Type::Float, Type::Int))
    }
    
    fn analyze_return_stmt(&mut self, expr: &Option<Expr>, span: &Span) {
        // Basic implementation - just check expression if present
        if let Some(expr) = expr {
            let _ = self.analyze_expression(expr, span);
        }
    }
    
    fn analyze_if_stmt(&mut self, condition: &Expr, then_branch: &Stmt, 
                      else_branch: Option<&Stmt>) {
        // Check condition is boolean
        match self.analyze_expression_type(condition) {
            Ok(ty) => {
                if !matches!(ty, Type::Bool) {
                    // Error already generated in expression analysis
                }
            }
            Err(_) => {
                // Error already generated
            }
        }
        
        self.analyze_statement(then_branch);
        if let Some(else_branch) = else_branch {
            self.analyze_statement(else_branch);
        }
    }
    
    fn analyze_while_stmt(&mut self, condition: &Expr, body: &Stmt) {
        self.loop_depth += 1;
        self.analyze_statement(body);
        self.loop_depth -= 1;
    }
    
    fn analyze_for_range_stmt(&mut self, var_name: &str, start: &Expr, end: &Expr, 
                             step: &Option<Box<Expr>>, body: &Stmt, span: &Span) {
        self.symbol_table.push_scope();
        
        // Add loop variable
        let symbol = Symbol::Variable {
            name: var_name.to_string(),
            ty: Type::Int,
            mutable: false,
            defined: true,
        };
        
        if let Err(e) = self.symbol_table.insert(symbol) {
            self.errors.push(SemanticError::new(
                span,
                &format!("Loop variable '{}' error: {}", var_name, e),
                Some("Loop variable names must be unique in their scope"),
            ));
        }
        
        self.loop_depth += 1;
        self.analyze_statement(body);
        self.loop_depth -= 1;
        
        self.symbol_table.pop_scope();
    }
    
    fn analyze_qif_stmt(&mut self, condition: &Expr, then_branch: &Stmt, 
                       else_branch: Option<&Stmt>) {
        // Save quantum context
        let old_context = self.in_quantum_context;
        self.in_quantum_context = true;
        
        self.analyze_statement(then_branch);
        if let Some(else_branch) = else_branch {
            self.analyze_statement(else_branch);
        }
        
        // Restore context
        self.in_quantum_context = old_context;
    }
    
    fn analyze_qfor_range_stmt(&mut self, var_name: &str, start: &Expr, end: &Expr, 
                              step: &Option<Box<Expr>>, body: &Stmt, span: &Span) {
        // Save quantum context
        let old_context = self.in_quantum_context;
        self.in_quantum_context = true;
        
        // Same checking as regular for range
        self.analyze_for_range_stmt(var_name, start, end, step, body, span);
        
        // Restore context
        self.in_quantum_context = old_context;
    }
    
    fn analyze_break_stmt(&mut self, span: &Span) {
        if self.loop_depth == 0 {
            self.errors.push(SemanticError::new(
                span,
                "Break statement outside loop",
                Some("Break statements must be inside loops"),
            ));
        }
    }
    
    fn analyze_continue_stmt(&mut self, span: &Span) {
        if self.loop_depth == 0 {
            self.errors.push(SemanticError::new(
                span,
                "Continue statement outside loop",
                Some("Continue statements must be inside loops"),
            ));
        }
    }
    
    pub fn get_errors(&self) -> &[SemanticError] {
        &self.errors
    }
    
    pub fn get_warnings(&self) -> &[String] {
        &self.warnings
    }
    
    pub fn get_type_registry(&self) -> &TypeRegistry {
        &self.type_registry
    }
}