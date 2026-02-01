
// ir.rs - COMPLETE FOR PHASE 1.3
use crate::ast::*;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub enum QIRGate {
    H,
    X,
    Y,
    Z,
    CNOT,
    RX(f64),
    RY(f64),
    RZ(f64),
    T,
    S,
    SWAP,
}

#[derive(Debug, Clone, PartialEq)]
pub enum QIRExpr {
    Qubit(BitString),
    Variable(String),
    GateApply(QIRGate, Vec<QIRExpr>),
    Measure(Box<QIRExpr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum QIRStmt {
    InitQubit(String, QIRExpr),
    ApplyGate(String, QIRExpr),
    MeasureQubit(String, String),
    ClassicalAssign(String, String),
    Return(String),
    Block(Vec<QIRStmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct QIRFunction {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub return_type: Type,
    pub body: Vec<QIRStmt>,
    pub qubit_count: usize,
    pub cbit_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QIRProgram {
    pub functions: Vec<QIRFunction>,
}

pub struct IRGenerator {
    qubit_counter: usize,
    cbit_counter: usize,
    all_qubits: HashSet<String>,
    all_cbits: HashSet<String>,
    current_qubit_names: HashMap<String, String>,
    current_cbit_names: HashMap<String, String>,
    loop_variables: HashMap<String, i64>,
    struct_fields: HashMap<String, HashMap<String, String>>, // struct_name -> field_name -> actual_name
}

impl IRGenerator {
    pub fn new() -> Self {
        Self {
            qubit_counter: 0,
            cbit_counter: 0,
            all_qubits: HashSet::new(),
            all_cbits: HashSet::new(),
            current_qubit_names: HashMap::new(),
            current_cbit_names: HashMap::new(),
            loop_variables: HashMap::new(),
            struct_fields: HashMap::new(),
        }
    }

    pub fn generate(&mut self, program: &Program) -> QIRProgram {
        let mut functions = Vec::new();
        
        for func in &program.functions {
            if let Some(qir_func) = self.generate_function(func) {
                functions.push(qir_func);
            }
        }
        
        QIRProgram { functions }
    }

    fn generate_function(&mut self, func: &Function) -> Option<QIRFunction> {
        self.qubit_counter = 0;
        self.cbit_counter = 0;
        self.all_qubits.clear();
        self.all_cbits.clear();
        self.current_qubit_names.clear();
        self.current_cbit_names.clear();
        self.loop_variables.clear();
        self.struct_fields.clear();
        
        let mut body = Vec::new();
        
        for stmt in &func.body {
            body.extend(self.generate_stmt(stmt));
        }
        
        let qubit_count = self.all_qubits.len();
        let cbit_count = self.all_cbits.len();
        
        Some(QIRFunction {
            name: func.name.clone(),
            params: func.params.iter().map(|p| (p.name.clone(), p.ty.clone())).collect(),
            return_type: func.return_type.clone(),
            body,
            qubit_count,
            cbit_count,
        })
    }

    fn generate_stmt(&mut self, stmt: &Stmt) -> Vec<QIRStmt> {
        match stmt {
            Stmt::Let(name, ty, expr, mutable, _span) => {
                self.generate_let_stmt(name, ty, expr, *mutable)
            }
            
            Stmt::Assign(name, expr, _span) => {
                self.generate_assign_stmt(name, expr)
            }
            
            Stmt::Expr(expr, _span) => {
                match expr {
                    Expr::BinaryOp(left, BinaryOp::Assign, right, _) => {
                        self.handle_assignment_expr(left, right)
                    }
                    Expr::GateApply(gate, args, _) => {
                        self.handle_standalone_gate(gate, args)
                    }
                    Expr::Measure(qubit_expr, _) => {
                        let temp_name = format!("temp_c{}", self.cbit_counter);
                        self.handle_measurement(&temp_name, qubit_expr).unwrap_or_default()
                    }
                    _ => vec![],
                }
            }
            
            Stmt::Return(expr, _) => {
                let value = expr.as_ref()
                    .and_then(|e| self.expr_to_string(e))
                    .unwrap_or_else(|| "0".to_string());
                vec![QIRStmt::Return(value)]
            }
            
            Stmt::Block(stmts, _) => {
                let mut result = Vec::new();
                for stmt in stmts {
                    result.extend(self.generate_stmt(stmt));
                }
                result
            }
            
            Stmt::If(condition, then_branch, else_branch, _span) => {
                self.generate_if_stmt(condition, then_branch, else_branch.as_deref())
            }
            
            Stmt::ForRange(var_name, start_expr, end_expr, step_expr, body_stmt, _span) => {
                self.generate_for_range_stmt(var_name, start_expr, end_expr, step_expr, body_stmt)
            }
            
            Stmt::TypeAlias(_, _) | Stmt::StructDef(_, _) => {
                vec![]
            }
            
            _ => vec![],
        }
    }
    
    fn handle_standalone_gate(&mut self, gate: &Gate, args: &[Expr]) -> Vec<QIRStmt> {
        let mut result = Vec::new();
        
        // For standalone gates, apply them to the arguments directly
        match gate {
            Gate::H | Gate::X | Gate::Y | Gate::Z | Gate::RX(_) | 
            Gate::RY(_) | Gate::RZ(_) | Gate::T | Gate::S => {
                if let Some(arg) = args.first() {
                    let qubit_name = self.extract_qubit_name(arg);
                    if let Some(actual_name) = qubit_name {
                        // Handle the None case explicitly instead of using ?
                        let qir_gate = match self.convert_gate(gate) {
                            Some(gate) => gate,
                            None => return Vec::new(), // Return empty vector if gate conversion fails
                        };
                        let qir_expr = QIRExpr::GateApply(qir_gate, vec![QIRExpr::Variable(actual_name.clone())]);
                        result.push(QIRStmt::ApplyGate(actual_name.clone(), qir_expr));
                    }
                }
            }
            Gate::CNOT => {
                if args.len() == 2 {
                    let ctrl_name = self.extract_qubit_name(&args[0]);
                    let target_name = self.extract_qubit_name(&args[1]);
                    
                    if let (Some(ctrl_actual), Some(target_actual)) = (ctrl_name, target_name) {
                        // Handle the None case explicitly instead of using ?
                        let qir_gate = match self.convert_gate(gate) {
                            Some(gate) => gate,
                            None => return Vec::new(), // Return empty vector if gate conversion fails
                        };
                        let qir_expr = QIRExpr::GateApply(qir_gate, vec![
                            QIRExpr::Variable(ctrl_actual.clone()),
                            QIRExpr::Variable(target_actual.clone())
                        ]);
                        
                        // For CNOT, apply to control qubit
                        result.push(QIRStmt::ApplyGate(ctrl_actual.clone(), qir_expr));
                    }
                }
            }
            _ => {}
        }
        
        result
    }
    
    fn extract_qubit_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Variable(name, _) => {
                self.current_qubit_names.get(name).cloned()
            }
            Expr::MemberAccess(base, field, _) => {
                if let Expr::Variable(struct_name, _) = &**base {
                    let full_name = format!("{}.{}", struct_name, field);
                    self.current_qubit_names.get(&full_name).cloned()
                } else {
                    None
                }
            }
            _ => None,
        }
    }
    
    fn handle_assignment_expr(&mut self, left: &Expr, right: &Expr) -> Vec<QIRStmt> {
        let target_name = match left {
            Expr::Variable(name, _) => name.clone(),
            Expr::Index(array_expr, index_expr, _) => {
                if let (Expr::Variable(array_name, _), Expr::LiteralInt(index, _)) = (&**array_expr, &**index_expr) {
                    format!("{}[{}]", array_name, index)
                } else {
                    return vec![];
                }
            }
            Expr::MemberAccess(base, field, _) => {
                // Handle struct member access
                if let Expr::Variable(struct_name, _) = &**base {
                    format!("{}.{}", struct_name, field)
                } else {
                    return vec![];
                }
            }
            _ => return vec![],
        };
        
        self.generate_assign_stmt(&target_name, right)
    }
    
    fn generate_let_stmt(&mut self, name: &str, ty: &Type, expr: &Expr, _mutable: bool) -> Vec<QIRStmt> {
        let mut result = Vec::new();
        
        match ty {
            Type::Qubit => {
                if let Expr::LiteralQubit(bit_string, _) = expr {
                    let unique_name = format!("q{}", self.qubit_counter);
                    self.qubit_counter += 1;
                    self.all_qubits.insert(unique_name.clone());
                    self.current_qubit_names.insert(name.to_string(), unique_name.clone());
                    
                    result.push(QIRStmt::InitQubit(unique_name, QIRExpr::Qubit(bit_string.clone())));
                } else if let Expr::GateApply(gate, args, _) = expr {
                    if let Some(stmts) = self.handle_gate_application(name, gate, args, true) {
                        result.extend(stmts);
                    }
                } else if let Expr::MemberAccess(struct_expr, field, _) = expr {
                    // Handle struct member initialization
                    if let Expr::Variable(struct_name, _) = &**struct_expr {
                        if let Some(struct_map) = self.struct_fields.get(struct_name) {
                            if let Some(field_name) = struct_map.get(field) {
                                // Copy the field value
                                if let Some(_source_name) = self.current_qubit_names.get(field_name) {
                                    let new_name = format!("q{}", self.qubit_counter);
                                    self.qubit_counter += 1;
                                    self.all_qubits.insert(new_name.clone());
                                    self.current_qubit_names.insert(name.to_string(), new_name.clone());
                                    
                                    // For now, just initialize as new qubit
                                    result.push(QIRStmt::InitQubit(
                                        new_name, 
                                        QIRExpr::Qubit(BitString::new(vec![0], Span::default()))
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            
            Type::Cbit => {
                if let Expr::Measure(qubit_expr, _) = expr {
                    if let Some(stmts) = self.handle_measurement(name, qubit_expr) {
                        result.extend(stmts);
                    }
                } else {
                    let value = self.expr_to_string(expr).unwrap_or_else(|| "0".to_string());
                    result.push(QIRStmt::ClassicalAssign(name.to_string(), value));
                }
            }
            
            Type::Qreg(size) => {
                if let Expr::LiteralQubit(bit_string, _) = expr {
                    for i in 0..*size {
                        let qubit_name = format!("{}[{}]", name, i);
                        let unique_name = format!("q{}", self.qubit_counter);
                        self.qubit_counter += 1;
                        self.all_qubits.insert(unique_name.clone());
                        
                        self.current_qubit_names.insert(qubit_name, unique_name.clone());
                        
                        let bit = if i < bit_string.bits.len() { bit_string.bits[i] } else { 0 };
                        let single_bit_string = BitString::new(vec![bit], Span::default());
                        
                        result.push(QIRStmt::InitQubit(unique_name, QIRExpr::Qubit(single_bit_string)));
                    }
                }
            }
            
            Type::Array(elem_type, size) => {
                for i in 0..*size {
                    let elem_name = format!("{}[{}]", name, i);
                    let default_value = match **elem_type {
                        Type::Int => "0".to_string(),
                        Type::Float => "0.0".to_string(),
                        Type::Bool => "false".to_string(),
                        Type::String => "".to_string(),
                        Type::Cbit => "0".to_string(),
                        _ => "0".to_string(),
                    };
                    result.push(QIRStmt::ClassicalAssign(elem_name, default_value));
                }
            }
            
            Type::Named(_struct_name) => {
                // Handle struct initialization
                if let Expr::StructLiteral(_, fields, _) = expr {
                    let mut field_map = HashMap::new();
                    
                    for (field_name, field_expr) in fields {
                        match field_expr {
                            Expr::LiteralQubit(bit_string, _) => {
                                let unique_name = format!("q{}", self.qubit_counter);
                                self.qubit_counter += 1;
                                self.all_qubits.insert(unique_name.clone());
                                
                                let full_field_name = format!("{}.{}", name, field_name);
                                self.current_qubit_names.insert(full_field_name.clone(), unique_name.clone());
                                field_map.insert(field_name.clone(), unique_name.clone());
                                
                                // Check if we need to apply X gate for |1>
                                if bit_string.bits.len() == 1 && bit_string.bits[0] == 1 {
                                    result.push(QIRStmt::InitQubit(
                                        unique_name.clone(), 
                                        QIRExpr::Qubit(BitString::new(vec![0], Span::default()))
                                    ));
                                    let qir_gate = QIRGate::X;
                                    let qir_expr = QIRExpr::GateApply(qir_gate, vec![QIRExpr::Variable(unique_name.clone())]);
                                    result.push(QIRStmt::ApplyGate(unique_name.clone(), qir_expr));
                                } else {
                                    result.push(QIRStmt::InitQubit(unique_name, QIRExpr::Qubit(bit_string.clone())));
                                }
                            }
                            Expr::LiteralInt(val, _) => {
                                let full_field_name = format!("{}.{}", name, field_name);
                                result.push(QIRStmt::ClassicalAssign(full_field_name, val.to_string()));
                            }
                            Expr::LiteralFloat(val, _) => {
                                let full_field_name = format!("{}.{}", name, field_name);
                                result.push(QIRStmt::ClassicalAssign(full_field_name, val.to_string()));
                            }
                            Expr::UnaryOp(UnaryOp::Neg, operand, _) => {
                                if let Expr::LiteralFloat(val, _) = &**operand {
                                    let full_field_name = format!("{}.{}", name, field_name);
                                    result.push(QIRStmt::ClassicalAssign(full_field_name, format!("-{}", val)));
                                }
                            }
                            _ => {
                                // Handle other field types
                                let value = self.expr_to_string(field_expr).unwrap_or_else(|| "0".to_string());
                                let full_field_name = format!("{}.{}", name, field_name);
                                result.push(QIRStmt::ClassicalAssign(full_field_name, value));
                            }
                        }
                    }
                    
                    self.struct_fields.insert(name.to_string(), field_map);
                }
            }
            
            _ => {
                if let Some(value) = self.expr_to_string(expr) {
                    result.push(QIRStmt::ClassicalAssign(name.to_string(), value));
                }
            }
        }
        
        result
    }
    
    fn generate_assign_stmt(&mut self, name: &str, expr: &Expr) -> Vec<QIRStmt> {
        let mut result = Vec::new();
        
        match expr {
            Expr::GateApply(gate, args, _) => {
                if let Some(stmts) = self.handle_quantum_assignment(name, gate, args) {
                    result.extend(stmts);
                }
            }
            
            Expr::LiteralQubit(bit_string, _) => {
                let unique_name = if let Some(existing) = self.current_qubit_names.get(name) {
                    existing.clone()
                } else {
                    let new_name = format!("q{}", self.qubit_counter);
                    self.qubit_counter += 1;
                    self.all_qubits.insert(new_name.clone());
                    self.current_qubit_names.insert(name.to_string(), new_name.clone());
                    new_name
                };
                
                result.push(QIRStmt::InitQubit(
                    unique_name, 
                    QIRExpr::Qubit(bit_string.clone())
                ));
            }
            
            Expr::LiteralInt(val, _) => {
                result.push(QIRStmt::ClassicalAssign(name.to_string(), val.to_string()));
            }
            
            Expr::LiteralFloat(val, _) => {
                result.push(QIRStmt::ClassicalAssign(name.to_string(), val.to_string()));
            }
            
            Expr::UnaryOp(UnaryOp::Neg, operand, _) => {
                if let Expr::LiteralFloat(val, _) = &**operand {
                    result.push(QIRStmt::ClassicalAssign(name.to_string(), format!("-{}", val)));
                }
            }
            
            _ => {
                if let Some(value) = self.expr_to_string(expr) {
                    result.push(QIRStmt::ClassicalAssign(name.to_string(), value));
                }
            }
        }
        
        result
    }
    
    fn handle_quantum_assignment(
        &mut self, 
        target_name: &str, 
        gate: &Gate, 
        args: &[Expr]
    ) -> Option<Vec<QIRStmt>> {
        let mut result = Vec::new();
        let mut qir_args = Vec::new();
        
        for arg in args {
            let qubit_name = self.extract_qubit_name(arg);
            if let Some(actual_arg_name) = qubit_name {
                qir_args.push(QIRExpr::Variable(actual_arg_name.clone()));
            }
        }
        
        let qir_gate = self.convert_gate(gate)?;
        
        let target_qubit_name = if let Some(existing_name) = self.current_qubit_names.get(target_name) {
            existing_name.clone()
        } else {
            let new_name = format!("q{}", self.qubit_counter);
            self.qubit_counter += 1;
            self.all_qubits.insert(new_name.clone());
            self.current_qubit_names.insert(target_name.to_string(), new_name.clone());
            new_name
        };
        
        let qir_expr = QIRExpr::GateApply(qir_gate, qir_args);
        result.push(QIRStmt::ApplyGate(target_qubit_name, qir_expr));
        
        Some(result)
    }
    
    fn handle_gate_application(
        &mut self, 
        target_name: &str, 
        gate: &Gate, 
        args: &[Expr], 
        is_new_qubit: bool
    ) -> Option<Vec<QIRStmt>> {
        let mut result = Vec::new();
        let mut qir_args = Vec::new();
        
        for arg in args {
            let qubit_name = self.extract_qubit_name(arg);
            if let Some(actual_arg_name) = qubit_name {
                qir_args.push(QIRExpr::Variable(actual_arg_name.clone()));
            }
        }
        
        let qir_gate = self.convert_gate(gate)?;
        
        if is_new_qubit {
            let output_name = format!("q{}", self.qubit_counter);
            self.qubit_counter += 1;
            self.all_qubits.insert(output_name.clone());
            self.current_qubit_names.insert(target_name.to_string(), output_name.clone());
            
            let qir_expr = QIRExpr::GateApply(qir_gate, qir_args);
            result.push(QIRStmt::ApplyGate(output_name, qir_expr));
        } else {
            let target_qubit_name = if let Some(existing_name) = self.current_qubit_names.get(target_name) {
                existing_name.clone()
            } else {
                let new_name = format!("q{}", self.qubit_counter);
                self.qubit_counter += 1;
                self.all_qubits.insert(new_name.clone());
                self.current_qubit_names.insert(target_name.to_string(), new_name.clone());
                new_name
            };
            
            let qir_expr = QIRExpr::GateApply(qir_gate, qir_args);
            result.push(QIRStmt::ApplyGate(target_qubit_name, qir_expr));
        }
        
        Some(result)
    }
    
    fn generate_for_range_stmt(
        &mut self, 
        var_name: &str, 
        start_expr: &Expr,
        end_expr: &Expr,
        step_expr: &Option<Box<Expr>>,
        body_stmt: &Stmt
    ) -> Vec<QIRStmt> {
        let start = self.evaluate_int_expr(start_expr).unwrap_or(0);
        let end = self.evaluate_int_expr(end_expr).unwrap_or(0);
        let step = step_expr.as_ref()
            .and_then(|s| self.evaluate_int_expr(s))
            .unwrap_or(1);
        
        if start >= end {
            return vec![];
        }
        
        let mut result = Vec::new();
        
        for i in (start..end).step_by(step as usize) {
            self.loop_variables.insert(var_name.to_string(), i);
            
            let body_result = self.generate_stmt(body_stmt);
            
            if !body_result.is_empty() {
                result.extend(body_result);
            }
            
            self.loop_variables.remove(var_name);
        }
        
        result
    }
    
    fn generate_if_stmt(
        &mut self,
        _condition: &Expr,
        then_branch: &Stmt,
        else_branch: Option<&Stmt>
    ) -> Vec<QIRStmt> {
        let mut result = Vec::new();
        
        let then_result = self.generate_stmt(then_branch);
        if !then_result.is_empty() {
            result.extend(then_result);
        }
        
        if let Some(else_branch) = else_branch {
            let else_result = self.generate_stmt(else_branch);
            if !else_result.is_empty() {
                result.extend(else_result);
            }
        }
        
        result
    }
    
    fn handle_measurement(&mut self, cbit_name: &str, qubit_expr: &Expr) -> Option<Vec<QIRStmt>> {
        let mut result = Vec::new();
        
        let qubit_name = self.extract_qubit_name(qubit_expr);
        
        if let Some(actual_qubit_name) = qubit_name {
            let unique_cbit_name = format!("c{}", self.cbit_counter);
            self.cbit_counter += 1;
            self.all_cbits.insert(unique_cbit_name.clone());
            
            self.current_cbit_names.insert(cbit_name.to_string(), unique_cbit_name.clone());
            
            result.push(QIRStmt::MeasureQubit(
                actual_qubit_name.clone(),
                unique_cbit_name
            ));
            
            return Some(result);
        }
        
        None
    }
    
    fn expr_to_string(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::LiteralInt(val, _) => Some(val.to_string()),
            Expr::LiteralFloat(val, _) => Some(val.to_string()),
            Expr::LiteralBool(val, _) => Some(val.to_string()),
            Expr::LiteralString(val, _) => Some(val.clone()),
            Expr::Variable(name, _) => {
                if let Some(val) = self.loop_variables.get(name) {
                    Some(val.to_string())
                } else {
                    Some(name.clone())
                }
            }
            Expr::BinaryOp(left, op, right, _) => {
                let left_val = self.expr_to_string(&**left)?;
                let right_val = self.expr_to_string(&**right)?;
                
                match op {
                    BinaryOp::Add => Some(format!("({} + {})", left_val, right_val)),
                    BinaryOp::Sub => Some(format!("({} - {})", left_val, right_val)),
                    BinaryOp::Mul => Some(format!("({} * {})", left_val, right_val)),
                    BinaryOp::Div => Some(format!("({} / {})", left_val, right_val)),
                    BinaryOp::Eq => Some(format!("({} == {})", left_val, right_val)),
                    BinaryOp::Neq => Some(format!("({} != {})", left_val, right_val)),
                    BinaryOp::Lt => Some(format!("({} < {})", left_val, right_val)),
                    BinaryOp::Gt => Some(format!("({} > {})", left_val, right_val)),
                    BinaryOp::Le => Some(format!("({} <= {})", left_val, right_val)),
                    BinaryOp::Ge => Some(format!("({} >= {})", left_val, right_val)),
                    BinaryOp::And => Some(format!("({} && {})", left_val, right_val)),
                    BinaryOp::Or => Some(format!("({} || {})", left_val, right_val)),
                    BinaryOp::AddAssign => Some(format!("({} += {})", left_val, right_val)),
                    BinaryOp::SubAssign => Some(format!("({} -= {})", left_val, right_val)),
                    BinaryOp::MulAssign => Some(format!("({} *= {})", left_val, right_val)),
                    BinaryOp::DivAssign => Some(format!("({} /= {})", left_val, right_val)),
                    _ => Some(format!("{} {}", left_val, right_val)),
                }
            }
            Expr::UnaryOp(op, operand, _) => {
                let operand_val = self.expr_to_string(operand)?;
                match op {
                    UnaryOp::Neg => Some(format!("-{}", operand_val)),
                    UnaryOp::Not => Some(format!("!{}", operand_val)),
                    _ => Some(operand_val),
                }
            }
            _ => None,
        }
    }
    
    fn evaluate_int_expr(&self, expr: &Expr) -> Option<i64> {
        match expr {
            Expr::LiteralInt(val, _) => Some(*val),
            Expr::Variable(name, _) => {
                self.loop_variables.get(name).copied()
            }
            Expr::BinaryOp(left, op, right, _) => {
                let left_val = self.evaluate_int_expr(&**left)?;
                let right_val = self.evaluate_int_expr(&**right)?;
                
                match op {
                    BinaryOp::Add => Some(left_val + right_val),
                    BinaryOp::Sub => Some(left_val - right_val),
                    BinaryOp::Mul => Some(left_val * right_val),
                    BinaryOp::Div => {
                        if right_val != 0 {
                            Some(left_val / right_val)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
    
    fn convert_gate(&self, gate: &Gate) -> Option<QIRGate> {
        match gate {
            Gate::H => Some(QIRGate::H),
            Gate::X => Some(QIRGate::X),
            Gate::Y => Some(QIRGate::Y),
            Gate::Z => Some(QIRGate::Z),
            Gate::CNOT => Some(QIRGate::CNOT),
            Gate::RX(angle_expr) => {
                if let Some(angle) = self.evaluate_float_expr(&**angle_expr) {
                    Some(QIRGate::RX(angle))
                } else {
                    None
                }
            }
            Gate::RY(angle_expr) => {
                if let Some(angle) = self.evaluate_float_expr(&**angle_expr) {
                    Some(QIRGate::RY(angle))
                } else {
                    None
                }
            }
            Gate::RZ(angle_expr) => {
                if let Some(angle) = self.evaluate_float_expr(&**angle_expr) {
                    Some(QIRGate::RZ(angle))
                } else {
                    None
                }
            }
            Gate::T => Some(QIRGate::T),
            Gate::S => Some(QIRGate::S),
            Gate::SWAP => Some(QIRGate::SWAP),
        }
    }
    
    fn evaluate_float_expr(&self, expr: &Expr) -> Option<f64> {
        match expr {
            Expr::LiteralInt(val, _) => Some(*val as f64),
            Expr::LiteralFloat(val, _) => Some(*val),
            Expr::Variable(name, _) => {
                self.loop_variables.get(name).map(|&v| v as f64)
            }
            Expr::BinaryOp(left, op, right, _) => {
                let left_val = self.evaluate_float_expr(&**left)?;
                let right_val = self.evaluate_float_expr(&**right)?;
                
                match op {
                    BinaryOp::Add => Some(left_val + right_val),
                    BinaryOp::Sub => Some(left_val - right_val),
                    BinaryOp::Mul => Some(left_val * right_val),
                    BinaryOp::Div => {
                        if right_val != 0.0 {
                            Some(left_val / right_val)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}
