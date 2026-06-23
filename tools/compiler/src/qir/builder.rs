// src/qir/builder.rs - FIXED LET STATEMENT HANDLER
use crate::ast::{Program, Function, Stmt, Expr, Type, BinaryOp, UnaryOp, Gate as AstGate};
// Removed: use crate::semantics::{SemanticAnalyzer, TypeRegistry}; -- We trust the caller!
use super::*;
use std::collections::HashMap;

pub struct QirBuilder {
    module: QirModule,
    current_function: Option<String>,
    // type_registry: TypeRegistry, // Removed dependency on TypeRegistry for now to simplify
    symbol_table: HashMap<String, (QirType, QirValue)>,
    loop_stack: Vec<BlockId>,
    qubit_counter: usize,
    cbit_counter: usize,
    temp_counter: usize,
}

impl QirBuilder {
    pub fn new() -> Self {
        Self {
            module: QirModule::new("main"),
            current_function: None,
            // type_registry: TypeRegistry::new(),
            symbol_table: HashMap::new(),
            loop_stack: Vec::new(),
            qubit_counter: 0,
            cbit_counter: 0,
            temp_counter: 0,
        }
    }
    
    pub fn build_from_program(&mut self, program: &Program) -> QirModule {
        // FIX: Removed redundant SemanticAnalyzer check. 
        // We assume lib.rs has already validated the AST.
        
        for func in &program.functions {
            self.build_function(func);
        }
        
        self.module.clone()
    }
    
    fn build_function(&mut self, ast_func: &Function) {
        let params: Vec<QirParam> = ast_func.params.iter().map(|p| {
            let qir_type = self.convert_type(&p.ty);
            QirParam {
                name: p.name.clone(),
                ty: qir_type,
                mutable: p.mutable,
            }
        }).collect();
        
        let return_type = self.convert_type(&ast_func.return_type);
        
        let mut qir_func = QirFunction::new(&ast_func.name, params, return_type);
        
        self.current_function = Some(ast_func.name.clone());
        self.symbol_table.clear();
        self.qubit_counter = 0;
        self.cbit_counter = 0;
        self.temp_counter = 0;
        
        for stmt in &ast_func.body {
            self.build_statement(stmt, &mut qir_func);
        }
        
        // Ensure the function ends with a return if not present (implicit void return)
        let current_blk = qir_func.get_current_block_mut();
        if !current_blk.is_terminated() {
            current_blk.add_op(QirOp::Return { value: None });
        }
        
        self.module.add_function(qir_func);
        self.current_function = None;
    }
    
    fn build_statement(&mut self, stmt: &Stmt, qir_func: &mut QirFunction) {
        match stmt {
            Stmt::Let(name, ty, expr, mutable, _) => {
                self.build_let_stmt(name, ty, expr, *mutable, qir_func);
            }
            Stmt::Assign(name, expr, _) => {
                self.build_assign_stmt(name, expr, qir_func);
            }
            Stmt::Expr(expr, _) => {
                self.build_expr(expr, qir_func);
            }
            Stmt::Return(expr, _) => {
                self.build_return_stmt(expr, qir_func);
            }
            Stmt::Block(stmts, _) => {
                self.build_block(stmts, qir_func);
            }
            Stmt::If(condition, then_branch, else_branch, _) => {
                self.build_if_stmt(condition, then_branch, else_branch.as_deref(), qir_func);
            }
            Stmt::While(condition, body, _) => {
                self.build_while_stmt(condition, body, qir_func);
            }
            Stmt::ForRange(var_name, start, end, step, body, _) => {
                self.build_for_range_stmt(var_name, start, end, step, body, qir_func);
            }
            Stmt::Break(_) => {
                self.build_break_stmt(qir_func);
            }
            Stmt::Continue(_) => {
                self.build_continue_stmt(qir_func);
            }
            _ => {}
        }
    }
    
    fn build_let_stmt(&mut self, name: &str, ty: &Type, expr: &Expr, _mutable: bool, qir_func: &mut QirFunction) {
        match ty {
            Type::Qreg(size) => {
                let mut qubit_values = Vec::new();
                let bit_string = if let Expr::LiteralQubit(bit_str, _) = expr {
                    Some(bit_str)
                } else {
                    None
                };
                
                for i in 0..*size {
                    let qubit_id = QubitId::new(self.qubit_counter);
                    self.qubit_counter += 1;
                    
                    let temp_id = TempId::new(self.temp_counter);
                    self.temp_counter += 1;
                    
                    let init_state = if let Some(bit_str) = &bit_string {
                        if i < bit_str.bits.len() && bit_str.bits[i] == 1 {
                            Some(BitState::One)
                        } else {
                            Some(BitState::Zero)
                        }
                    } else {
                        Some(BitState::Zero)
                    };
                    
                    qir_func.add_op(QirOp::AllocQubit {
                        result: temp_id,
                        init_state,
                    });
                    
                    qubit_values.push(QirValue::Qubit(qubit_id));
                }
                
                let qir_type = self.convert_type(ty);
                self.symbol_table.insert(name.to_string(), (qir_type, QirValue::Array(qubit_values)));
            }
            Type::Array(elem_type, size) => {
                if let Type::Cbit = **elem_type {
                    let mut cbit_values = Vec::new();
                    
                    for _ in 0..*size {
                        let cbit_id = CbitId::new(self.cbit_counter);
                        self.cbit_counter += 1;
                        
                        let temp_id = TempId::new(self.temp_counter);
                        self.temp_counter += 1;
                        
                        qir_func.add_op(QirOp::AllocCbit {
                            result: temp_id,
                            init_value: Some(0),
                        });
                        
                        cbit_values.push(QirValue::Cbit(cbit_id));
                    }
                    
                    let qir_type = self.convert_type(ty);
                    self.symbol_table.insert(name.to_string(), (qir_type, QirValue::Array(cbit_values)));
                } else {
                    let value = self.build_expr_value(expr, qir_func);
                    let qir_type = self.convert_type(ty);
                    self.symbol_table.insert(name.to_string(), (qir_type, value));
                }
            }
            _ => {
                let value = self.build_expr_value(expr, qir_func);
                let qir_type = self.convert_type(ty);
                self.symbol_table.insert(name.to_string(), (qir_type, value));
            }
        }
    }
    
    fn build_assign_stmt(&mut self, name: &str, expr: &Expr, qir_func: &mut QirFunction) {
        if let Some(left_bracket) = name.find('[') {
            if let Some(right_bracket) = name.find(']') {
                let array_name = &name[..left_bracket];
                let index_str = &name[left_bracket + 1..right_bracket];
                
                if let Ok(index) = index_str.parse::<usize>() {
                    let new_value = self.build_expr_value(expr, qir_func);
                    
                    if let Some((_, array_value)) = self.symbol_table.get_mut(array_name) {
                        if let QirValue::Array(elements) = array_value {
                            if index < elements.len() {
                                elements[index] = new_value;
                            }
                        }
                    }
                }
            }
        }
    }
    
    fn build_expr(&mut self, expr: &Expr, qir_func: &mut QirFunction) -> QirValue {
        self.build_expr_value(expr, qir_func)
    }
    
    fn build_expr_value(&mut self, expr: &Expr, qir_func: &mut QirFunction) -> QirValue {
        match expr {
            Expr::LiteralInt(value, _) => QirValue::Int(*value),
            Expr::LiteralFloat(value, _) => QirValue::Float(*value),
            Expr::LiteralBool(value, _) => QirValue::Bool(*value),
            Expr::LiteralString(value, _) => QirValue::String(value.clone()),
            Expr::LiteralQubit(bit_string, _) => {
                let qubit_id = QubitId::new(self.qubit_counter);
                self.qubit_counter += 1;
                let temp_id = TempId::new(self.temp_counter);
                self.temp_counter += 1;
                
                let init_state = if bit_string.bits.len() == 1 && bit_string.bits[0] == 1 {
                    Some(BitState::One)
                } else {
                    Some(BitState::Zero)
                };
                
                qir_func.add_op(QirOp::AllocQubit {
                    result: temp_id,
                    init_state,
                });
                
                QirValue::Qubit(qubit_id)
            }
            Expr::Variable(name, _) => {
                if let Some((_ty, value)) = self.symbol_table.get(name) {
                    value.clone()
                } else {
                    QirValue::Variable(name.clone())
                }
            }
            Expr::BinaryOp(left, op, right, _) => {
                self.build_binary_expr(left, op, right, qir_func)
            }
            Expr::UnaryOp(op, operand, _) => {
                self.build_unary_expr(op, operand, qir_func)
            }
            Expr::Call(name, args, _) => {
                self.build_call_expr(name, args, qir_func)
            }
            Expr::Measure(qubit_expr, _) => {
                self.build_measure_expr(qubit_expr, qir_func)
            }
            Expr::GateApply(gate, args, _) => {
                self.build_gate_apply_expr(gate, args, qir_func)
            }
            Expr::Index(array_expr, index_expr, _) => {
                self.build_index_expr(array_expr, index_expr, qir_func)
            }
            Expr::Tuple(elements, _) => {
                let values: Vec<QirValue> = elements.iter()
                    .map(|e| self.build_expr_value(e, qir_func))
                    .collect();
                QirValue::Tuple(values)
            }
            _ => QirValue::Null,
        }
    }
    
    fn build_binary_expr(&mut self, left: &Expr, op: &BinaryOp, right: &Expr, qir_func: &mut QirFunction) -> QirValue {
        let lhs = self.build_expr_value(left, qir_func);
        let rhs = self.build_expr_value(right, qir_func);
        let result_temp = TempId::new(self.temp_counter);
        self.temp_counter += 1;
        
        qir_func.add_op(QirOp::BinaryOp {
            op: op.clone(),
            lhs,
            rhs,
            result: result_temp,
        });
        QirValue::Temp(result_temp)
    }
    
    fn build_unary_expr(&mut self, op: &UnaryOp, operand: &Expr, qir_func: &mut QirFunction) -> QirValue {
        let operand_val = self.build_expr_value(operand, qir_func);
        let result_temp = TempId::new(self.temp_counter);
        self.temp_counter += 1;
        
        qir_func.add_op(QirOp::UnaryOp {
            op: op.clone(),
            operand: operand_val,
            result: result_temp,
        });
        QirValue::Temp(result_temp)
    }
    
    fn build_call_expr(&mut self, name: &str, args: &[Expr], qir_func: &mut QirFunction) -> QirValue {
        match name {
            "range" => {
                if args.len() >= 2 {
                    let start = self.build_expr_value(&args[0], qir_func);
                    let end = self.build_expr_value(&args[1], qir_func);
                    QirValue::Tuple(vec![start, end])
                } else {
                    QirValue::Null
                }
            }
            "measure" => {
                if let Some(arg) = args.first() {
                    self.build_measure_expr(arg, qir_func)
                } else {
                    QirValue::Null
                }
            }
            _ => QirValue::Null,
        }
    }
    
    fn build_measure_expr(&mut self, qubit_expr: &Expr, qir_func: &mut QirFunction) -> QirValue {
        let value = self.build_expr_value(qubit_expr, qir_func);

        if let QirValue::Qubit(qubit_id) = value {
            let cbit_id = CbitId::new(self.cbit_counter);
            self.cbit_counter += 1;
            
            qir_func.add_op(QirOp::Measure {
                qubit: qubit_id,
                cbit: cbit_id,
            });
            
            return QirValue::Cbit(cbit_id);
        }

        // Fallback for manual array resolution
        if let Expr::Index(array_expr, index_expr, _) = qubit_expr {
            if let Expr::Variable(array_name, _) = array_expr.as_ref() {
                if let Some((_, array_value)) = self.symbol_table.get(array_name) {
                    if let QirValue::Array(elements) = array_value {
                        let idx = if let Expr::LiteralInt(index, _) = index_expr.as_ref() {
                            *index as usize
                        } else if let Expr::Variable(var_name, _) = index_expr.as_ref() {
                            if let Some((_, QirValue::Int(i))) = self.symbol_table.get(var_name) {
                                *i as usize
                            } else {
                                return QirValue::Null;
                            }
                        } else {
                            return QirValue::Null;
                        };
                        
                        if idx < elements.len() {
                            if let QirValue::Qubit(qubit_id) = &elements[idx] {
                                let cbit_id = CbitId::new(self.cbit_counter);
                                self.cbit_counter += 1;
                                
                                qir_func.add_op(QirOp::Measure {
                                    qubit: *qubit_id,
                                    cbit: cbit_id,
                                });
                                
                                return QirValue::Cbit(cbit_id);
                            }
                        }
                    }
                }
            }
        }
        
        QirValue::Null
    }
    
    fn build_gate_apply_expr(&mut self, gate: &AstGate, args: &[Expr], qir_func: &mut QirFunction) -> QirValue {
        let mut arg_values = Vec::new();
        let mut first_qubit = None;
        
        for arg in args {
            let value = self.build_expr_value(arg, qir_func);
            let actual_value = if matches!(value, QirValue::Null) {
                if let Expr::Index(array_expr, index_expr, _) = arg {
                     self.build_index_expr(array_expr, index_expr, qir_func)
                } else {
                    value
                }
            } else {
                value
            };

            if first_qubit.is_none() {
                if let QirValue::Qubit(qubit_id) = &actual_value {
                    first_qubit = Some(*qubit_id);
                }
            }
            arg_values.push(actual_value);
        }
        
        if let Some(qir_gate) = QirGate::from_ast_gate(gate) {
            let result_temp = TempId::new(self.temp_counter);
            self.temp_counter += 1;
            
            qir_func.add_op(QirOp::ApplyGate {
                gate: qir_gate,
                args: arg_values,
                result: Some(result_temp),
            });
            
            if let Some(qubit_id) = first_qubit {
                return QirValue::Qubit(qubit_id);
            }
        }
        
        QirValue::Null
    }
    
    fn build_index_expr(&mut self, array_expr: &Expr, index_expr: &Expr, qir_func: &mut QirFunction) -> QirValue {
        let array_val = self.build_expr_value(array_expr, qir_func);
        let index_val = self.build_expr_value(index_expr, qir_func);
        
        if let (QirValue::Array(elements), QirValue::Int(index)) = (array_val.clone(), index_val.clone()) {
            let idx = index as usize;
            if idx < elements.len() {
                return elements[idx].clone();
            }
        }
        
        if let (QirValue::Variable(array_name), QirValue::Int(index)) = (array_val, index_val) {
            if let Some((_, array_value)) = self.symbol_table.get(&array_name) {
                if let QirValue::Array(elements) = array_value {
                    let idx = index as usize;
                    if idx < elements.len() {
                        return elements[idx].clone();
                    }
                }
            }
        }
        
        QirValue::Null
    }
    
    fn build_member_access_expr(&mut self, _base_expr: &Expr, _field: &str, _qir_func: &mut QirFunction) -> QirValue {
        QirValue::Null
    }
    
    fn build_return_stmt(&mut self, expr: &Option<Expr>, qir_func: &mut QirFunction) {
        let value = expr.as_ref()
            .map(|e| self.build_expr_value(e, qir_func))
            .unwrap_or(QirValue::Null);
        
        qir_func.add_op(QirOp::Return {
            value: if value == QirValue::Null { None } else { Some(value) },
        });
    }
    
    fn build_block(&mut self, stmts: &[Stmt], qir_func: &mut QirFunction) {
        for stmt in stmts {
            self.build_statement(stmt, qir_func);
        }
    }
    
    fn build_if_stmt(&mut self, condition: &Expr, then_branch: &Stmt, else_branch: Option<&Stmt>, qir_func: &mut QirFunction) {
        let _cond_val = self.build_expr_value(condition, qir_func);
        self.build_statement(then_branch, qir_func);
        if let Some(else_branch) = else_branch {
            self.build_statement(else_branch, qir_func);
        }
    }
    
    fn build_while_stmt(&mut self, condition: &Expr, body: &Stmt, qir_func: &mut QirFunction) {
        let _cond_val = self.build_expr_value(condition, qir_func);
        self.build_statement(body, qir_func);
    }
    
    fn build_for_range_stmt(&mut self, var_name: &str, start: &Expr, end: &Expr, 
                           _step: &Option<Box<Expr>>, body: &Stmt, qir_func: &mut QirFunction) {
        let start_val = self.build_expr_value(start, qir_func);
        let end_val = self.build_expr_value(end, qir_func);
        
        if let (QirValue::Int(start_int), QirValue::Int(end_int)) = (start_val, end_val) {
            for i in start_int..end_int {
                self.symbol_table.insert(var_name.to_string(), (QirType::Int, QirValue::Int(i)));
                self.build_statement(body, qir_func);
            }
        }
    }
    
    fn build_break_stmt(&mut self, _qir_func: &mut QirFunction) {}
    
    fn build_continue_stmt(&mut self, _qir_func: &mut QirFunction) {}
    
    fn convert_type(&self, ast_type: &Type) -> QirType {
        match ast_type {
            Type::Int => QirType::Int,
            Type::Float => QirType::Float,
            Type::Bool => QirType::Bool,
            Type::String => QirType::String,
            Type::Qubit => QirType::Qubit,
            Type::Cbit => QirType::Cbit,
            Type::Qreg(size) => QirType::Qreg(*size),
            Type::Array(elem_type, size) => {
                QirType::Array(Box::new(self.convert_type(elem_type)), *size)
            }
            Type::Unit => QirType::Unit,
            Type::Tuple(types) => {
                QirType::Tuple(types.iter().map(|t| self.convert_type(t)).collect())
            }
            Type::Named(name) => {
                QirType::Struct(name.clone(), Vec::new())
            }
            _ => QirType::Unit,
        }
    }
}