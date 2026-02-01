// parser.rs - COMPLETE FOR PHASE 1.3
use crate::lexer::Token;
use crate::lexer::is_gate_name;
use crate::ast::*;
use std::iter::Peekable;
use std::fmt;

#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub column: usize,
    pub hint: Option<String>,
}

#[derive(Debug)]
pub struct Parser<I: Iterator<Item = (Token, usize, usize)> + Clone> {
    tokens: Peekable<I>,
    pub errors: Vec<ParseError>,
    source: String,
    position: usize,
    type_aliases: std::collections::HashMap<String, Type>,
    struct_defs: std::collections::HashMap<String, StructDef>,
}

impl<I: Iterator<Item = (Token, usize, usize)> + Clone> Parser<I> {
    pub fn new(tokens: I, source: String) -> Self {
        Self {
            tokens: tokens.peekable(),
            errors: Vec::new(),
            source,
            position: 0,
            type_aliases: std::collections::HashMap::new(),
            struct_defs: std::collections::HashMap::new(),
        }
    }

    pub fn parse_program(&mut self) -> Program {
        let mut functions = Vec::new();
        let mut type_aliases = Vec::new();
        let mut struct_defs = Vec::new();
        
        while self.peek_token().is_some() {
            match self.peek_token() {
                Some(Token::KwType) => {
                    if let Some(Stmt::TypeAlias(alias, _)) = self.parse_stmt() {
                        type_aliases.push(alias.clone());
                        self.type_aliases.insert(alias.name.clone(), alias.target.clone());
                    }
                }
                Some(Token::KwStruct) => {
                    if let Some(Stmt::StructDef(struct_def, _)) = self.parse_stmt() {
                        struct_defs.push(struct_def.clone());
                        self.struct_defs.insert(struct_def.name.clone(), struct_def);
                    }
                }
                Some(Token::KwFn) => {
                    if let Some(func) = self.parse_function() {
                        functions.push(func);
                    } else {
                        self.recover_to_next_function();
                    }
                }
                _ => {
                    self.recover_to_next_function();
                }
            }
        }
        
        Program { 
            functions,
            type_aliases,
            struct_defs,
            source: Some(self.source.clone()),
        }
    }
    
    fn parse_function(&mut self) -> Option<Function> {
        let start_pos = self.position;
        let (start_line, start_col) = match self.peek_token_with_pos() {
            Some((_, line, col)) => (*line, *col),
            None => return None,
        };
        
        self.expect(&Token::KwFn, "function declaration")?;
        
        let name = match self.expect_ident("function name") {
            Some(name) => name,
            None => return None,
        };
        
        self.expect(&Token::ParenOpen, "opening parenthesis for parameters")?;
        let params = self.parse_params();
        self.expect(&Token::ParenClose, "closing parenthesis for parameters")?;
        
        self.expect(&Token::Arrow, "return type arrow '->'")?;
        let return_type = match self.parse_type() {
            Some(ty) => ty,
            None => {
                self.add_error(
                    "Expected return type after '->'".to_string(),
                    self.position,
                    0,
                    Some("Add a return type like 'int', 'qubit', or 'unit'".to_string()),
                );
                return None;
            }
        };
        
        self.expect(&Token::BraceOpen, "opening brace for function body")?;
        
        let body = match self.parse_block_statements() {
            Some(stmts) => stmts,
            None => {
                self.add_error(
                    "Expected function body".to_string(),
                    self.position,
                    0,
                    Some("Add statements inside the function body".to_string()),
                );
                return None;
            }
        };
        
        self.expect(&Token::BraceClose, "closing brace for function body")?;
        
        let end_pos = self.position;
        let span = Span::new(start_line, start_col, start_pos, end_pos);
        
        Some(Function {
            name,
            params,
            return_type,
            body,
            span,
        })
    }

    fn parse_params(&mut self) -> Vec<Param> {
        let mut params = Vec::new();
        
        if self.peek_token() == Some(&Token::ParenClose) {
            return params;
        }
        
        loop {
            let (param_line, param_col) = match self.peek_token_with_pos() {
                Some((_, line, col)) => (*line, *col),
                None => break,
            };
            let param_start = self.position;
            
            let mutable = self.consume_if(&Token::KwMut);
            
            let (name, ty) = if self.peek_is_type() {
                let ty = self.parse_type().unwrap_or(Type::Unit);
                let name = match self.expect_ident("parameter name") {
                    Some(name) => name,
                    None => break,
                };
                (name, ty)
            } else {
                let name = match self.expect_ident("parameter name") {
                    Some(name) => name,
                    None => break,
                };
                self.expect(&Token::Colon, "colon after parameter name");
                let ty = self.parse_type().unwrap_or(Type::Unit);
                (name, ty)
            };
            
            let param_span = Span::new(param_line, param_col, param_start, self.position);
            
            params.push(Param { 
                name, 
                ty, 
                mutable,
                span: param_span,
            });
            
            if !self.consume_if(&Token::Comma) {
                break;
            }
        }
        
        params
    }

    fn parse_type(&mut self) -> Option<Type> {
        let (token, line, col) = self.next_token()?;
        
        match token {
            Token::KwInt => Some(Type::Int),
            Token::KwFloat => Some(Type::Float),
            Token::KwBool => Some(Type::Bool),
            Token::KwString => Some(Type::String),
            Token::KwQubit => Some(Type::Qubit),
            Token::KwCbit => Some(Type::Cbit),
            Token::KwQreg => {
                self.expect(&Token::BracketOpen, "opening bracket for qreg size")?;
                let size = match self.parse_int_literal() {
                    Some(n) => n as usize,
                    None => return None,
                };
                self.expect(&Token::BracketClose, "closing bracket for qreg size")?;
                Some(Type::Qreg(size))
            }
            Token::ParenOpen => {
                // Check if this is a tuple type or just parentheses
                if self.peek_token() == Some(&Token::ParenClose) {
                    // Empty tuple: () -> unit type
                    self.expect(&Token::ParenClose, "closing parenthesis for empty tuple")?;
                    Some(Type::Unit)
                } else {
                    // Parse first type
                    let first_type = self.parse_type()?;
                    
                    // Check if there's a comma (then it's a tuple)
                    if self.consume_if(&Token::Comma) {
                        let mut types = vec![first_type];
                        
                        // Parse remaining types
                        while self.peek_token() != Some(&Token::ParenClose) {
                            if let Some(ty) = self.parse_type() {
                                types.push(ty);
                            } else {
                                break;
                            }
                            
                            if !self.consume_if(&Token::Comma) {
                                break;
                            }
                        }
                        
                        self.expect(&Token::ParenClose, "closing parenthesis for tuple type")?;
                        Some(Type::Tuple(types))
                    } else {
                        // Just parentheses around a single type
                        self.expect(&Token::ParenClose, "closing parenthesis")?;
                        Some(first_type)
                    }
                }
            }
            Token::Ident(name) => {
                // Check if this is a type alias
                if let Some(aliased_type) = self.type_aliases.get(&name) {
                    Some(aliased_type.clone())
                } else if self.struct_defs.contains_key(&name) {
                    Some(Type::Named(name))
                } else {
                    // Could be a simple named type
                    Some(Type::Named(name))
                }
            }
            _ => {
                self.add_error(
                    format!(
                        "Expected type, found '{}'",
                        self.token_to_string(&token)
                    ),
                    line,
                    col,
                    Some("Try: int, float, bool, string, qubit, cbit, qreg[...], (type1, type2, ...), or a type alias".to_string()),
                );
                None
            }
        }
    }

    fn parse_type_alias_stmt(&mut self) -> Option<Stmt> {
        let start_pos = self.position;
        let (start_line, start_col) = match self.peek_token_with_pos() {
            Some((_, line, col)) => (*line, *col),
            None => return None,
        };
        
        // Consume the 'type' keyword
        self.expect(&Token::KwType, "'type' keyword")?;
        
        let name = self.expect_ident("type alias name")?;
        self.expect(&Token::OpAssign, "'=' in type alias")?;
        
        let target = self.parse_type()?;
        self.expect(&Token::Semicolon, "semicolon after type alias")?;
        
        let span = Span::new(start_line, start_col, start_pos, self.position);
        let type_alias = TypeAlias {
            name,
            target,
            span: span.clone(),
        };
        
        Some(Stmt::TypeAlias(type_alias, span))
    }

    fn parse_struct_def_stmt(&mut self) -> Option<Stmt> {
        let start_pos = self.position;
        let (start_line, start_col) = match self.peek_token_with_pos() {
            Some((_, line, col)) => (*line, *col),
            None => return None,
        };
        
        // Consume the 'struct' keyword
        self.expect(&Token::KwStruct, "'struct' keyword")?;
        
        let name = self.expect_ident("struct name")?;
        self.expect(&Token::BraceOpen, "opening brace for struct definition")?;
        
        let mut fields = Vec::new();
        
        while self.peek_token() != Some(&Token::BraceClose) && self.peek_token().is_some() {
            let (field_line, field_col) = match self.peek_token_with_pos() {
                Some((_, line, col)) => (*line, *col),
                None => break,
            };
            let field_start = self.position;
            
            let field_name = self.expect_ident("struct field name")?;
            self.expect(&Token::Colon, "colon after field name")?;
            let field_type = self.parse_type()?;
            
            self.consume_if(&Token::Comma);
            
            let field_span = Span::new(field_line, field_col, field_start, self.position);
            fields.push(StructField {
                name: field_name,
                ty: field_type,
                span: field_span,
            });
        }
        
        self.expect(&Token::BraceClose, "closing brace for struct definition")?;
        self.expect(&Token::Semicolon, "semicolon after struct definition")?;
        
        let span = Span::new(start_line, start_col, start_pos, self.position);
        let struct_def = StructDef {
            name,
            fields,
            span: span.clone(),
        };
        
        Some(Stmt::StructDef(struct_def, span))
    }

    fn parse_block_statements(&mut self) -> Option<Vec<Stmt>> {
        let mut stmts = Vec::new();
        
        while self.peek_token() != Some(&Token::BraceClose) && self.peek_token().is_some() {
            if let Some(stmt) = self.parse_stmt() {
                stmts.push(stmt);
            } else {
                self.recover_in_block();
            }
        }
        
        Some(stmts)
    }

    fn parse_stmt(&mut self) -> Option<Stmt> {
        let start_pos = self.position;
        let (start_line, start_col) = match self.peek_token_with_pos() {
            Some((_, line, col)) => (*line, *col),
            None => return None,
        };
        
        let stmt = match self.peek_token() {
            Some(Token::KwLet) => self.parse_let_stmt(),
            Some(Token::KwType) => self.parse_type_alias_stmt(),
            Some(Token::KwStruct) => self.parse_struct_def_stmt(),
            Some(Token::KwInt) => self.parse_old_style_var_decl_stmt(false),
            Some(Token::KwFloat) => self.parse_old_style_var_decl_stmt(false),
            Some(Token::KwBool) => self.parse_old_style_var_decl_stmt(false),
            Some(Token::KwString) => self.parse_old_style_var_decl_stmt(false),
            Some(Token::KwQubit) => self.parse_old_style_var_decl_stmt(false),
            Some(Token::KwCbit) => self.parse_old_style_var_decl_stmt(false),
            Some(Token::KwQreg) => self.parse_qreg_stmt(),
            Some(Token::KwIf) => self.parse_if_stmt(),
            Some(Token::KwWhile) => self.parse_while_stmt(),
            Some(Token::KwFor) => self.parse_for_range_stmt(),
            Some(Token::KwBreak) => self.parse_break_stmt(),
            Some(Token::KwContinue) => self.parse_continue_stmt(),
            Some(Token::KwReturn) => self.parse_return_stmt(),
            Some(Token::KwQIf) => self.parse_qif_stmt(),
            Some(Token::KwQFor) => self.parse_qfor_range_stmt(),
            Some(Token::BraceOpen) => self.parse_block_stmt(),
            Some(Token::KwMut) => self.parse_mut_var_decl_stmt(),
            
            // Check if identifier is a type alias or struct name
            Some(Token::Ident(ref name)) => {
                let name_clone = name.clone();
                if self.type_aliases.contains_key(&name_clone) || 
                   self.struct_defs.contains_key(&name_clone) {
                    self.parse_old_style_var_decl_stmt(false)
                } else {
                    self.parse_expr_stmt()
                }
            }
            
            // Check if '(' starts a tuple type
            Some(Token::ParenOpen) => {
                // We need to check if this is a tuple type without consuming tokens
                let saved_tokens = self.tokens.clone();
                let saved_position = self.position;
                
                let mut temp_parser = Parser {
                    tokens: saved_tokens,
                    errors: Vec::new(),
                    source: self.source.clone(),
                    position: saved_position,
                    type_aliases: self.type_aliases.clone(),
                    struct_defs: self.struct_defs.clone(),
                };
                
                if let Some(_) = temp_parser.parse_type() {
                    if let Some(Token::Ident(_)) = temp_parser.peek_token() {
                        self.parse_old_style_var_decl_stmt(false)
                    } else {
                        self.parse_expr_stmt()
                    }
                } else {
                    self.parse_expr_stmt()
                }
            }
            
            _ => self.parse_expr_stmt(),
        };
        
        if let Some(stmt) = stmt {
            let span = Span::new(start_line, start_col, start_pos, self.position);
            Some(self.add_span_to_stmt(stmt, span))
        } else {
            None
        }
    }

    fn parse_mut_var_decl_stmt(&mut self) -> Option<Stmt> {
        let peek_result = self.peek_token_with_pos().cloned();
        let (_, line, col) = match peek_result {
            Some((token, l, c)) => (token, l, c),
            None => return None,
        };
        
        self.expect(&Token::KwMut, "'mut' keyword")?;
        
        let ty = self.parse_type()?;
        
        match ty {
            Type::Qubit | Type::Qreg(_) => {
                self.add_error(
                    format!("Quantum type {:?} cannot be mutable", ty),
                    line,
                    col,
                    Some("Quantum resources follow affine typing rules and cannot be reassigned".to_string()),
                );
                return None;
            }
            _ => {}
        }
        
        let name = self.expect_ident("variable name")?;
        
        let (actual_ty, array_size) = if self.consume_if(&Token::BracketOpen) {
            let size = match self.parse_int_literal() {
                Some(n) => n as usize,
                None => return None,
            };
            self.expect(&Token::BracketClose, "closing bracket for array size")?;
            
            match ty {
                Type::Cbit => (Type::Array(Box::new(Type::Cbit), size), Some(size)),
                Type::Int => (Type::Array(Box::new(Type::Int), size), Some(size)),
                Type::Float => (Type::Array(Box::new(Type::Float), size), Some(size)),
                Type::Bool => (Type::Array(Box::new(Type::Bool), size), Some(size)),
                Type::String => (Type::Array(Box::new(Type::String), size), Some(size)),
                _ => {
                    self.add_error(
                        format!("Arrays of type {:?} are not supported", ty),
                        line,
                        col,
                        Some("Only cbit, int, float, bool, and string arrays are supported".to_string()),
                    );
                    return None;
                }
            }
        } else {
            (ty, None)
        };
        
        if !self.consume_if(&Token::OpAssign) {
            self.expect(&Token::Semicolon, "semicolon after variable declaration")?;
            
            let default_expr = if let Some(_size) = array_size {
                let expr_span = Span::new(line, col, self.position, self.position);
                Expr::LiteralInt(0, expr_span)
            } else {
                let expr_span = Span::new(line, col, self.position, self.position);
                match actual_ty {
                    Type::Int => Expr::LiteralInt(0, expr_span),
                    Type::Float => Expr::LiteralFloat(0.0, expr_span),
                    Type::Bool => Expr::LiteralBool(false, expr_span),
                    Type::String => Expr::LiteralString("".to_string(), expr_span),
                    Type::Cbit => Expr::LiteralInt(0, expr_span),
                    Type::Qubit => {
                        self.add_error(
                            "Qubit must be initialized with |0> or |1>".to_string(),
                            line,
                            col,
                            Some("Use: qubit q = |0>; or qubit q = |1>;".to_string()),
                        );
                        return None;
                    }
                    _ => Expr::LiteralInt(0, expr_span),
                }
            };
            
            return Some(Stmt::Let(name, actual_ty, default_expr, true, Span::new(line, col, self.position, self.position)));
        }
        
        let expr = self.parse_expr()?;
        self.expect(&Token::Semicolon, "semicolon after variable initialization")?;
        
        Some(Stmt::Let(name, actual_ty, expr, true, Span::new(line, col, self.position, self.position)))
    }

    fn parse_old_style_var_decl_stmt(&mut self, mutable: bool) -> Option<Stmt> {
        let start_pos = self.position;
        let (start_line, start_col) = match self.peek_token_with_pos() {
            Some((_, line, col)) => (*line, *col),
            None => return None,
        };
        
        let ty = match self.parse_type() {
            Some(ty) => ty,
            None => {
                self.add_error(
                    "Expected type in variable declaration".to_string(),
                    start_line,
                    start_col,
                    Some("Try: int, float, bool, string, qubit, cbit, qreg[...], (type1, type2, ...), or a type alias".to_string()),
                );
                return None;
            }
        };
        
        let name = match self.expect_ident("variable name") {
            Some(name) => name,
            None => return None,
        };
        
        let (actual_ty, array_size) = if self.consume_if(&Token::BracketOpen) {
            let size = match self.parse_int_literal() {
                Some(n) => n as usize,
                None => return None,
            };
            self.expect(&Token::BracketClose, "closing bracket for array size")?;
            
            match &ty {
                Type::Cbit => (Type::Array(Box::new(Type::Cbit), size), Some(size)),
                Type::Int => (Type::Array(Box::new(Type::Int), size), Some(size)),
                Type::Float => (Type::Array(Box::new(Type::Float), size), Some(size)),
                Type::Bool => (Type::Array(Box::new(Type::Bool), size), Some(size)),
                Type::String => (Type::Array(Box::new(Type::String), size), Some(size)),
                Type::Named(alias_name) => {
                    if let Some(aliased_type) = self.type_aliases.get(alias_name) {
                        match aliased_type {
                            Type::Cbit => (Type::Array(Box::new(Type::Cbit), size), Some(size)),
                            Type::Int => (Type::Array(Box::new(Type::Int), size), Some(size)),
                            Type::Float => (Type::Array(Box::new(Type::Float), size), Some(size)),
                            Type::Bool => (Type::Array(Box::new(Type::Bool), size), Some(size)),
                            Type::String => (Type::Array(Box::new(Type::String), size), Some(size)),
                            _ => {
                                self.add_error(
                                    format!("Arrays of type '{}' are not supported", alias_name),
                                    start_line,
                                    start_col,
                                    Some("Only cbit, int, float, bool, and string arrays are supported".to_string()),
                                );
                                return None;
                            }
                        }
                    } else {
                        self.add_error(
                            format!("Arrays of type '{}' are not supported", alias_name),
                            start_line,
                            start_col,
                            Some("Only cbit, int, float, bool, and string arrays are supported".to_string()),
                        );
                        return None;
                    }
                }
                _ => {
                    self.add_error(
                        format!("Arrays of type {:?} are not supported", ty),
                        start_line,
                        start_col,
                        Some("Only cbit, int, float, bool, and string arrays are supported".to_string()),
                    );
                    return None;
                }
            }
        } else {
            (ty, None)
        };
        
        if !self.consume_if(&Token::OpAssign) {
            self.expect(&Token::Semicolon, "semicolon after variable declaration")?;
            
            let default_expr = if let Some(_size) = array_size {
                let expr_span = Span::new(start_line, start_col, self.position, self.position);
                Expr::LiteralInt(0, expr_span)
            } else {
                let expr_span = Span::new(start_line, start_col, self.position, self.position);
                match &actual_ty {
                    Type::Int => Expr::LiteralInt(0, expr_span),
                    Type::Float => Expr::LiteralFloat(0.0, expr_span),
                    Type::Bool => Expr::LiteralBool(false, expr_span),
                    Type::String => Expr::LiteralString("".to_string(), expr_span),
                    Type::Cbit => Expr::LiteralInt(0, expr_span),
                    Type::Qubit => {
                        self.add_error(
                            "Qubit must be initialized with |0> or |1>".to_string(),
                            start_line,
                            start_col,
                            Some("Use: qubit q = |0>; or qubit q = |1>;".to_string()),
                        );
                        return None;
                    }
                    Type::Qreg(size) => {
                        let bits = vec![0; *size];
                        let bit_string = BitString::new(bits, Span::default());
                        Expr::LiteralQubit(bit_string, expr_span)
                    }
                    Type::Named(_) => Expr::LiteralInt(0, expr_span),
                    Type::Tuple(_) => Expr::LiteralInt(0, expr_span),
                    Type::Unit => Expr::LiteralInt(0, expr_span),
                    _ => Expr::LiteralInt(0, expr_span),
                }
            };
            
            return Some(Stmt::Let(name, actual_ty, default_expr, mutable, 
                                Span::new(start_line, start_col, start_pos, self.position)));
        }
        
        let expr = self.parse_expr()?;
        self.expect(&Token::Semicolon, "semicolon after variable initialization")?;
        
        Some(Stmt::Let(name, actual_ty, expr, mutable, 
                      Span::new(start_line, start_col, start_pos, self.position)))
    }

    fn parse_qreg_stmt(&mut self) -> Option<Stmt> {
        let peek_result = self.peek_token_with_pos().cloned();
        let (_, line, col) = match peek_result {
            Some((token, l, c)) => (token, l, c),
            None => return None,
        };
        
        self.expect(&Token::KwQreg, "'qreg' keyword")?;
        
        let name = self.expect_ident("qreg name")?;
        
        self.expect(&Token::BracketOpen, "opening bracket for qreg size")?;
        let size = match self.parse_int_literal() {
            Some(n) => n as usize,
            None => return None,
        };
        self.expect(&Token::BracketClose, "closing bracket for qreg size")?;
        
        self.expect(&Token::OpAssign, "assignment operator '=' for qreg")?;
        
        let (bits, bits_line, bits_col) = match self.next_token()? {
            (Token::QubitLiteral(bits), l, c) => (bits, l, c),
            _ => {
                self.add_error(
                    "Expected bit string literal for qreg initialization".to_string(),
                    line,
                    col,
                    Some("Example: |000>".to_string()),
                );
                return None;
            }
        };
        
        if bits.bits.len() != size {
            self.add_error(
                format!("Bit string length {} doesn't match qreg size {}", bits.bits.len(), size),
                bits_line,
                bits_col,
                Some(format!("Expected {} bits, got {}", size, bits.bits.len())),
            );
            return None;
        }
        
        self.expect(&Token::Semicolon, "semicolon after qreg declaration")?;
        
        let bits_span = Span::new(bits_line, bits_col, self.position, self.position);
        let bit_string = BitString::new(bits.bits.clone(), bits_span);
        
        Some(Stmt::Let(
            name,
            Type::Qreg(size),
            Expr::LiteralQubit(bit_string, Span::new(line, col, self.position, self.position)),
            false,
            Span::new(line, col, self.position, self.position)
        ))
    }

fn parse_let_stmt(&mut self) -> Option<Stmt> {
    let peek_result = self.peek_token_with_pos().cloned();
    let (_, line, col) = match peek_result {
        Some((token, l, c)) => (token, l, c),
        None => return None,
    };
    
    self.expect(&Token::KwLet, "'let' keyword")?;
    
    let mutable = self.consume_if(&Token::KwMut);
    
    // Check if it's a tuple pattern
    if self.peek_token() == Some(&Token::ParenOpen) {
        // Parse tuple pattern: (ident, ident, ...)
        self.next_token(); // Skip '('
        
        let mut names = Vec::new();
        loop {
            let name = self.expect_ident("tuple pattern element")?;
            names.push(name);
            
            if !self.consume_if(&Token::Comma) {
                break;
            }
        }
        
        self.expect(&Token::ParenClose, "closing parenthesis for tuple pattern")?;
        self.expect(&Token::Colon, "colon after tuple pattern")?;
        
        // For now, assume it's a tuple type matching the pattern
        // This is simplified - you'd need proper type checking
        let ty = self.parse_type().unwrap_or(Type::Unit);
        
        self.expect(&Token::OpAssign, "assignment operator '='")?;
        let expr = self.parse_expr()?;
        
        self.expect(&Token::Semicolon, "semicolon after let statement")?;
        
        // Return a tuple destructuring statement
        // Note: You'll need to add a new Stmt variant for this
        // For now, we'll return a placeholder
        return Some(Stmt::Expr(expr, Span::new(line, col, self.position, self.position)));
    } else {
        // Original single variable parsing
        let name = self.expect_ident("variable name")?;
        self.expect(&Token::Colon, "colon after variable name")?;
        let ty = self.parse_type().unwrap_or(Type::Unit);
        
        if mutable {
            match ty {
                Type::Qubit | Type::Qreg(_) => {
                    self.add_error(
                        format!("Quantum type {:?} cannot be mutable", ty),
                        line,
                        col,
                        Some("Quantum resources follow affine typing rules and cannot be reassigned".to_string()),
                    );
                    return None;
                }
                _ => {}
            }
        }
        
        self.expect(&Token::OpAssign, "assignment operator '='")?;
        let expr = self.parse_expr()?;
        
        self.expect(&Token::Semicolon, "semicolon after let statement")?;
        
        Some(Stmt::Let(name, ty, expr, mutable, Span::new(line, col, self.position, self.position)))
    }
}

    fn parse_for_range_stmt(&mut self) -> Option<Stmt> {
        let peek_result = self.peek_token_with_pos().cloned();
        let (_, line, col) = match peek_result {
            Some((token, l, c)) => (token, l, c),
            None => return None,
        };
        
        self.expect(&Token::KwFor, "'for' keyword")?;
        
        let var_name = self.expect_ident("loop variable")?;
        self.expect(&Token::KwIn, "'in' keyword after loop variable")?;
        self.expect(&Token::KwRange, "'range' keyword")?;
        
        self.expect(&Token::ParenOpen, "opening parenthesis for range")?;
        let start_expr = self.parse_expr()?;
        self.expect(&Token::Comma, "comma between range arguments")?;
        let end_expr = self.parse_expr()?;
        
        let step_expr = if self.consume_if(&Token::Comma) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        
        self.expect(&Token::ParenClose, "closing parenthesis for range")?;
        let body = Box::new(self.parse_stmt()?);
        
        Some(Stmt::ForRange(var_name, Box::new(start_expr), Box::new(end_expr), step_expr, body, 
                          Span::new(line, col, self.position, self.position)))
    }

    fn parse_qfor_range_stmt(&mut self) -> Option<Stmt> {
        let peek_result = self.peek_token_with_pos().cloned();
        let (_, line, col) = match peek_result {
            Some((token, l, c)) => (token, l, c),
            None => return None,
        };
        
        self.expect(&Token::KwQFor, "'qfor' keyword")?;
        
        let var_name = self.expect_ident("loop variable")?;
        self.expect(&Token::KwIn, "'in' keyword after loop variable")?;
        self.expect(&Token::KwRange, "'range' keyword")?;
        
        self.expect(&Token::ParenOpen, "opening parenthesis for range")?;
        let start_expr = self.parse_expr()?;
        self.expect(&Token::Comma, "comma between range arguments")?;
        let end_expr = self.parse_expr()?;
        
        let step_expr = if self.consume_if(&Token::Comma) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        
        self.expect(&Token::ParenClose, "closing parenthesis for range")?;
        let body = Box::new(self.parse_stmt()?);
        
        Some(Stmt::QForRange(var_name, Box::new(start_expr), Box::new(end_expr), step_expr, body,
                           Span::new(line, col, self.position, self.position)))
    }

    fn parse_break_stmt(&mut self) -> Option<Stmt> {
        let peek_result = self.peek_token_with_pos().cloned();
        let (_, line, col) = match peek_result {
            Some((token, l, c)) => (token, l, c),
            None => return None,
        };
        
        self.expect(&Token::KwBreak, "'break' keyword")?;
        self.expect(&Token::Semicolon, "semicolon after break")?;
        Some(Stmt::Break(Span::new(line, col, self.position, self.position)))
    }

    fn parse_continue_stmt(&mut self) -> Option<Stmt> {
        let peek_result = self.peek_token_with_pos().cloned();
        let (_, line, col) = match peek_result {
            Some((token, l, c)) => (token, l, c),
            None => return None,
        };
        
        self.expect(&Token::KwContinue, "'continue' keyword")?;
        self.expect(&Token::Semicolon, "semicolon after continue")?;
        Some(Stmt::Continue(Span::new(line, col, self.position, self.position)))
    }

    fn parse_return_stmt(&mut self) -> Option<Stmt> {
        let peek_result = self.peek_token_with_pos().cloned();
        let (_, line, col) = match peek_result {
            Some((token, l, c)) => (token, l, c),
            None => return None,
        };
        
        self.expect(&Token::KwReturn, "'return' keyword")?;
        let expr = if self.peek_token() != Some(&Token::Semicolon) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(&Token::Semicolon, "semicolon after return")?;
        Some(Stmt::Return(expr, Span::new(line, col, self.position, self.position)))
    }

    fn parse_if_stmt(&mut self) -> Option<Stmt> {
        let peek_result = self.peek_token_with_pos().cloned();
        let (_, line, col) = match peek_result {
            Some((token, l, c)) => (token, l, c),
            None => return None,
        };
        
        self.expect(&Token::KwIf, "'if' keyword")?;
        
        let condition = if self.consume_if(&Token::ParenOpen) {
            let cond = self.parse_expr()?;
            self.expect(&Token::ParenClose, "closing parenthesis for condition")?;
            cond
        } else {
            self.parse_expr()?
        };
        
        let then_branch = Box::new(self.parse_stmt()?);
        let else_branch = if self.consume_if(&Token::KwElse) {
            Some(Box::new(self.parse_stmt()?))
        } else {
            None
        };
        
        Some(Stmt::If(condition, then_branch, else_branch, Span::new(line, col, self.position, self.position)))
    }

    fn parse_qif_stmt(&mut self) -> Option<Stmt> {
        let peek_result = self.peek_token_with_pos().cloned();
        let (_, line, col) = match peek_result {
            Some((token, l, c)) => (token, l, c),
            None => return None,
        };
        
        self.expect(&Token::KwQIf, "'qif' keyword")?;
        
        let condition = if self.consume_if(&Token::ParenOpen) {
            let cond = self.parse_expr()?;
            self.expect(&Token::ParenClose, "closing parenthesis for condition")?;
            cond
        } else {
            self.parse_expr()?
        };
        
        let then_branch = Box::new(self.parse_stmt()?);
        let else_branch = if self.consume_if(&Token::KwQElse) {
            Some(Box::new(self.parse_stmt()?))
        } else {
            None
        };
        
        Some(Stmt::QIf(Box::new(condition), then_branch, else_branch, Span::new(line, col, self.position, self.position)))
    }

    fn parse_while_stmt(&mut self) -> Option<Stmt> {
        let peek_result = self.peek_token_with_pos().cloned();
        let (_, line, col) = match peek_result {
            Some((token, l, c)) => (token, l, c),
            None => return None,
        };
        
        self.expect(&Token::KwWhile, "'while' keyword")?;
        self.expect(&Token::ParenOpen, "opening parenthesis for condition")?;
        let condition = self.parse_expr()?;
        self.expect(&Token::ParenClose, "closing parenthesis for condition")?;
        
        let body = Box::new(self.parse_stmt()?);
        Some(Stmt::While(condition, body, Span::new(line, col, self.position, self.position)))
    }

    fn parse_block_stmt(&mut self) -> Option<Stmt> {
        let peek_result = self.peek_token_with_pos().cloned();
        let (_, line, col) = match peek_result {
            Some((token, l, c)) => (token, l, c),
            None => return None,
        };
        
        self.expect(&Token::BraceOpen, "opening brace for block")?;
        let stmts = self.parse_block_statements()?;
        self.expect(&Token::BraceClose, "closing brace for block")?;
        Some(Stmt::Block(stmts, Span::new(line, col, self.position, self.position)))
    }

    fn parse_expr_stmt(&mut self) -> Option<Stmt> {
        let peek_result = self.peek_token_with_pos().cloned();
        let (_, line, col) = match peek_result {
            Some((token, l, c)) => (token, l, c),
            None => return None,
        };
        
        let expr = self.parse_expr()?;
        self.expect(&Token::Semicolon, "semicolon after expression")?;
        
        if let Expr::BinaryOp(ref lhs, BinaryOp::Assign, ref rhs, _) = &expr {
            if let Expr::Variable(var_name, _) = &**lhs {
                return Some(Stmt::Assign(var_name.clone(), (**rhs).clone(), 
                                       Span::new(line, col, self.position, self.position)));
            }
        }
        
        Some(Stmt::Expr(expr, Span::new(line, col, self.position, self.position)))
    }

    fn parse_expr(&mut self) -> Option<Expr> {
        self.parse_assignment_expr()
    }

    fn parse_assignment_expr(&mut self) -> Option<Expr> {
        let start_pos = self.position;
        let (start_line, start_col) = match self.peek_token_with_pos() {
            Some((_, line, col)) => (*line, *col),
            None => return None,
        };
        
        let lhs = self.parse_or_expr()?;
        
        if self.consume_if(&Token::OpAssign) {
            let rhs = self.parse_assignment_expr()?;
            let span = Span::new(start_line, start_col, start_pos, self.position);
            Some(Expr::BinaryOp(
                Box::new(lhs),
                BinaryOp::Assign,
                Box::new(rhs),
                span
            ))
        } else if self.consume_if(&Token::OpAddAssign) {
            let rhs = self.parse_assignment_expr()?;
            let span = Span::new(start_line, start_col, start_pos, self.position);
            Some(Expr::BinaryOp(
                Box::new(lhs),
                BinaryOp::AddAssign,
                Box::new(rhs),
                span
            ))
        } else if self.consume_if(&Token::OpSubAssign) {
            let rhs = self.parse_assignment_expr()?;
            let span = Span::new(start_line, start_col, start_pos, self.position);
            Some(Expr::BinaryOp(
                Box::new(lhs),
                BinaryOp::SubAssign,
                Box::new(rhs),
                span
            ))
        } else if self.consume_if(&Token::OpMulAssign) {
            let rhs = self.parse_assignment_expr()?;
            let span = Span::new(start_line, start_col, start_pos, self.position);
            Some(Expr::BinaryOp(
                Box::new(lhs),
                BinaryOp::MulAssign,
                Box::new(rhs),
                span
            ))
        } else if self.consume_if(&Token::OpDivAssign) {
            let rhs = self.parse_assignment_expr()?;
            let span = Span::new(start_line, start_col, start_pos, self.position);
            Some(Expr::BinaryOp(
                Box::new(lhs),
                BinaryOp::DivAssign,
                Box::new(rhs),
                span
            ))
        } else {
            Some(lhs)
        }
    }

    fn parse_or_expr(&mut self) -> Option<Expr> {
        let start_pos = self.position;
        let (start_line, start_col) = match self.peek_token_with_pos() {
            Some((_, line, col)) => (*line, *col),
            None => return None,
        };
        
        let mut expr = self.parse_and_expr()?;
        
        while self.peek_token() == Some(&Token::OpOr) {
            self.next_token();
            let rhs = self.parse_and_expr()?;
            let span = Span::new(start_line, start_col, start_pos, self.position);
            expr = Expr::BinaryOp(Box::new(expr), BinaryOp::Or, Box::new(rhs), span);
        }
        
        Some(expr)
    }

    fn parse_and_expr(&mut self) -> Option<Expr> {
        let start_pos = self.position;
        let (start_line, start_col) = match self.peek_token_with_pos() {
            Some((_, line, col)) => (*line, *col),
            None => return None,
        };
        
        let mut expr = self.parse_equality_expr()?;
        
        while self.peek_token() == Some(&Token::OpAnd) {
            self.next_token();
            let rhs = self.parse_equality_expr()?;
            let span = Span::new(start_line, start_col, start_pos, self.position);
            expr = Expr::BinaryOp(Box::new(expr), BinaryOp::And, Box::new(rhs), span);
        }
        
        Some(expr)
    }

    fn parse_equality_expr(&mut self) -> Option<Expr> {
        let start_pos = self.position;
        let (start_line, start_col) = match self.peek_token_with_pos() {
            Some((_, line, col)) => (*line, *col),
            None => return None,
        };
        
        let mut expr = self.parse_relational_expr()?;
        
        while let Some(op) = self.parse_equality_op() {
            let rhs = self.parse_relational_expr()?;
            let span = Span::new(start_line, start_col, start_pos, self.position);
            expr = Expr::BinaryOp(Box::new(expr), op, Box::new(rhs), span);
        }
        
        Some(expr)
    }

    fn parse_equality_op(&mut self) -> Option<BinaryOp> {
        match self.peek_token() {
            Some(Token::OpEq) => { self.next_token(); Some(BinaryOp::Eq) }
            Some(Token::OpNeq) => { self.next_token(); Some(BinaryOp::Neq) }
            _ => None,
        }
    }

    fn parse_relational_expr(&mut self) -> Option<Expr> {
        let start_pos = self.position;
        let (start_line, start_col) = match self.peek_token_with_pos() {
            Some((_, line, col)) => (*line, *col),
            None => return None,
        };
        
        let mut expr = self.parse_additive_expr()?;
        
        while let Some(op) = self.parse_relational_op() {
            let rhs = self.parse_additive_expr()?;
            let span = Span::new(start_line, start_col, start_pos, self.position);
            expr = Expr::BinaryOp(Box::new(expr), op, Box::new(rhs), span);
        }
        
        Some(expr)
    }

    fn parse_relational_op(&mut self) -> Option<BinaryOp> {
        match self.peek_token() {
            Some(Token::OpLt) => { self.next_token(); Some(BinaryOp::Lt) }
            Some(Token::OpGt) => { self.next_token(); Some(BinaryOp::Gt) }
            Some(Token::OpLe) => { self.next_token(); Some(BinaryOp::Le) }
            Some(Token::OpGe) => { self.next_token(); Some(BinaryOp::Ge) }
            _ => None,
        }
    }

    fn parse_additive_expr(&mut self) -> Option<Expr> {
        let start_pos = self.position;
        let (start_line, start_col) = match self.peek_token_with_pos() {
            Some((_, line, col)) => (*line, *col),
            None => return None,
        };
        
        let mut expr = self.parse_multiplicative_expr()?;
        
        while let Some(op) = self.parse_additive_op() {
            let rhs = self.parse_multiplicative_expr()?;
            let span = Span::new(start_line, start_col, start_pos, self.position);
            expr = Expr::BinaryOp(Box::new(expr), op, Box::new(rhs), span);
        }
        
        Some(expr)
    }

    fn parse_additive_op(&mut self) -> Option<BinaryOp> {
        match self.peek_token() {
            Some(Token::OpAdd) => { self.next_token(); Some(BinaryOp::Add) }
            Some(Token::OpSub) => { self.next_token(); Some(BinaryOp::Sub) }
            _ => None,
        }
    }

    fn parse_multiplicative_expr(&mut self) -> Option<Expr> {
        let start_pos = self.position;
        let (start_line, start_col) = match self.peek_token_with_pos() {
            Some((_, line, col)) => (*line, *col),
            None => return None,
        };
        
        let mut expr = self.parse_unary_expr()?;
        
        while let Some(op) = self.parse_multiplicative_op() {
            let rhs = self.parse_unary_expr()?;
            let span = Span::new(start_line, start_col, start_pos, self.position);
            expr = Expr::BinaryOp(Box::new(expr), op, Box::new(rhs), span);
        }
        
        Some(expr)
    }

    fn parse_multiplicative_op(&mut self) -> Option<BinaryOp> {
        match self.peek_token() {
            Some(Token::OpMul) => { self.next_token(); Some(BinaryOp::Mul) }
            Some(Token::OpDiv) => { self.next_token(); Some(BinaryOp::Div) }
            _ => None,
        }
    }

    fn parse_unary_expr(&mut self) -> Option<Expr> {
        let start_pos = self.position;
        let (start_line, start_col) = match self.peek_token_with_pos() {
            Some((_, line, col)) => (*line, *col),
            None => return None,
        };
        
        if let Some(op) = self.parse_unary_op() {
            let expr = self.parse_unary_expr()?;
            let span = Span::new(start_line, start_col, start_pos, self.position);
            Some(Expr::UnaryOp(op, Box::new(expr), span))
        } else {
            self.parse_primary_expr()
        }
    }

    fn parse_unary_op(&mut self) -> Option<UnaryOp> {
        match self.peek_token() {
            Some(Token::OpSub) => { self.next_token(); Some(UnaryOp::Neg) }
            Some(Token::OpNot) => { self.next_token(); Some(UnaryOp::Not) }
            _ => None,
        }
    }

    fn parse_primary_expr(&mut self) -> Option<Expr> {
        let (token, line, col) = self.next_token()?;
        
        match token {
            Token::IntLiteral(n) => {
                let span = Span::new(line, col, self.position, self.position);
                Some(Expr::LiteralInt(n, span))
            }
            Token::FloatLiteral(f) => {
                let span = Span::new(line, col, self.position, self.position);
                Some(Expr::LiteralFloat(f, span))
            }
            Token::StringLiteral(s) => {
                let span = Span::new(line, col, self.position, self.position);
                Some(Expr::LiteralString(s, span))
            }
            Token::QubitLiteral(bits) => {
                let span = Span::new(line, col, self.position, self.position);
                Some(Expr::LiteralQubit(bits, span))
            }
            Token::Ident(name) => {
                if self.peek_token() == Some(&Token::BraceOpen) {
                    self.parse_struct_literal(&name, line, col)
                } else if self.peek_token() == Some(&Token::ParenOpen) {
                    self.next_token();
                    let args = self.parse_args()?;
                    self.expect(&Token::ParenClose, "closing parenthesis for function call")?;
                    
                    let span = Span::new(line, col, self.position, self.position);
                    
                    if is_gate_name(&name) {
                        self.parse_gate_application(&name, args, span)
                    } else if name == "measure" {
                        if args.len() == 1 {
                            Some(Expr::Measure(Box::new(args[0].clone()), span))
                        } else {
                            self.add_error(
                                format!("measure expects 1 argument, got {}", args.len()),
                                line,
                                col,
                                Some("Usage: measure(qubit)".to_string()),
                            );
                            None
                        }
                    } else {
                        Some(Expr::Call(name, args, span))
                    }
                } else if self.peek_token() == Some(&Token::BracketOpen) {
                    let array_expr = Expr::Variable(name, Span::new(line, col, self.position, self.position));
                    self.next_token();
                    let index_expr = self.parse_expr()?;
                    self.expect(&Token::BracketClose, "closing bracket for array index")?;
                    
                    let span = Span::new(line, col, self.position, self.position);
                    Some(Expr::Index(Box::new(array_expr), Box::new(index_expr), span))
                } else {
                    let base_expr = Expr::Variable(name, Span::new(line, col, self.position, self.position));
                    self.parse_member_access(base_expr, line, col)
                }
            }
            Token::ParenOpen => {
                let first_expr = self.parse_expr()?;
                
                if self.consume_if(&Token::Comma) {
                    let mut elements = vec![first_expr];
                    
                    while self.peek_token() != Some(&Token::ParenClose) {
                        if let Some(expr) = self.parse_expr() {
                            elements.push(expr);
                        } else {
                            break;
                        }
                        
                        if !self.consume_if(&Token::Comma) {
                            break;
                        }
                    }
                    
                    self.expect(&Token::ParenClose, "closing parenthesis for tuple")?;
                    let span = Span::new(line, col, self.position, self.position);
                    Some(Expr::Tuple(elements, span))
                } else {
                    self.expect(&Token::ParenClose, "closing parenthesis")?;
                    Some(first_expr)
                }
            }
            _ => {
                self.add_error(
                    format!("Expected expression, found '{}'", self.token_to_string(&token)),
                    line,
                    col,
                    Some("Expected: number, string, variable, struct literal, or '('".to_string()),
                );
                None
            }
        }
    }

    fn parse_member_access(&mut self, base_expr: Expr, line: usize, col: usize) -> Option<Expr> {
        let mut current_expr = base_expr;
        
        while self.peek_token() == Some(&Token::Dot) {
            self.next_token(); // Consume the dot
            
            // Get the token after the dot
            let (token, token_line, token_col) = match self.next_token() {
                Some(t) => t,
                None => return None,
            };
            
            let field_name = match token {
                Token::IntLiteral(n) => n.to_string(),
                Token::Ident(name) => name,
                _ => {
                    self.add_error(
                        format!("Expected field name or tuple index after '.', found '{}'", 
                               self.token_to_string(&token)),
                        token_line,
                        token_col,
                        Some("Use '.field' for struct fields or '.0', .1, etc. for tuple elements".to_string()),
                    );
                    return None;
                }
            };
            
            let span = Span::new(line, col, self.position, self.position);
            current_expr = Expr::MemberAccess(Box::new(current_expr), field_name, span);
        }
        
        Some(current_expr)
    }

    fn parse_struct_literal(&mut self, struct_name: &str, line: usize, col: usize) -> Option<Expr> {
        self.expect(&Token::BraceOpen, "opening brace for struct literal")?;
        
        let mut fields = Vec::new();
        
        if self.peek_token() != Some(&Token::BraceClose) {
            loop {
                let field_name = self.expect_ident("struct field name")?;
                self.expect(&Token::Colon, "colon after field name")?;
                
                let value = self.parse_expr()?;
                fields.push((field_name, value));
                
                if !self.consume_if(&Token::Comma) {
                    break;
                }
                
                if self.peek_token() == Some(&Token::BraceClose) {
                    break;
                }
            }
        }
        
        self.expect(&Token::BraceClose, "closing brace for struct literal")?;
        
        let span = Span::new(line, col, self.position, self.position);
        Some(Expr::StructLiteral(struct_name.to_string(), fields, span))
    }

    fn parse_args(&mut self) -> Option<Vec<Expr>> {
        let mut args = Vec::new();
        
        if self.peek_token() == Some(&Token::ParenClose) {
            return Some(args);
        }
        
        loop {
            if let Some(expr) = self.parse_expr() {
                args.push(expr);
            } else {
                break;
            }
            
            if !self.consume_if(&Token::Comma) {
                break;
            }
        }
        
        Some(args)
    }
    
    fn parse_gate_application(&mut self, gate_name: &str, args: Vec<Expr>, span: Span) -> Option<Expr> {
        let gate_name_lower = gate_name.to_lowercase();
        let gate = match gate_name_lower.as_str() {
            "h" => Gate::H,
            "x" => Gate::X,
            "y" => Gate::Y,
            "z" => Gate::Z,
            "cnot" => Gate::CNOT,
            "t" => Gate::T,
            "s" => Gate::S,
            "swap" => Gate::SWAP,
            "rx" => {
                if args.len() == 2 {
                    let angle = args[0].clone();
                    Gate::RX(Box::new(angle))
                } else {
                    self.add_error(
                        format!("RX gate expects 2 arguments (angle and qubit), got {}", args.len()),
                        span.line,
                        span.column,
                        Some("Usage: RX(angle, qubit)".to_string()),
                    );
                    return None;
                }
            }
            "ry" => {
                if args.len() == 2 {
                    let angle = args[0].clone();
                    Gate::RY(Box::new(angle))
                } else {
                    self.add_error(
                        format!("RY gate expects 2 arguments (angle and qubit), got {}", args.len()),
                        span.line,
                        span.column,
                        Some("Usage: RY(angle, qubit)".to_string()),
                    );
                    return None;
                }
            }
            "rz" => {
                if args.len() == 2 {
                    let angle = args[0].clone();
                    Gate::RZ(Box::new(angle))
                } else {
                    self.add_error(
                        format!("RZ gate expects 2 arguments (angle and qubit), got {}", args.len()),
                        span.line,
                        span.column,
                        Some("Usage: RZ(angle, qubit)".to_string()),
                    );
                    return None;
                }
            }
            _ => {
                self.add_error(
                    format!("Unknown gate: '{}'", gate_name),
                    span.line,
                    span.column,
                    Some("Valid gates: H, X, Y, Z, CNOT, RX, RY, RZ, T, S, SWAP".to_string()),
                );
                return None;
            }
        };
        
        let gate_args = match gate {
            Gate::RX(_) | Gate::RY(_) | Gate::RZ(_) => {
                if args.len() == 2 {
                    vec![args[1].clone()]
                } else {
                    vec![]
                }
            }
            _ => args,
        };
        
        Some(Expr::GateApply(Box::new(gate), gate_args, span))
    }

    fn parse_int_literal(&mut self) -> Option<i64> {
        let (token, line, col) = self.next_token()?;
        match token {
            Token::IntLiteral(n) => Some(n),
            _ => {
                self.add_error(
                    format!("Expected integer literal, found '{}'", self.token_to_string(&token)),
                    line,
                    col,
                    Some("Example: 42, 0, 100".to_string()),
                );
                None
            }
        }
    }

    fn peek_is_type(&mut self) -> bool {
        let token = self.peek_token().cloned();
        
        match token {
            Some(Token::KwInt)
            | Some(Token::KwFloat)
            | Some(Token::KwBool)
            | Some(Token::KwString)
            | Some(Token::KwQubit)
            | Some(Token::KwCbit)
            | Some(Token::KwQreg)
            | Some(Token::ParenOpen) => true,
            
            Some(Token::Ident(name)) => {
                self.type_aliases.contains_key(&name) || self.struct_defs.contains_key(&name)
            }
            
            _ => false,
        }
    }

    fn add_error(&mut self, message: String, line: usize, column: usize, hint: Option<String>) {
        self.errors.push(ParseError {
            message,
            line,
            column,
            hint,
        });
    }
    
    fn recover_to_next_function(&mut self) {
        while let Some((token, _, _)) = self.tokens.next() {
            if matches!(token, Token::KwFn) {
                break;
            }
        }
    }
    
    fn recover_in_block(&mut self) {
        while let Some((token, _, _)) = self.peek_token_with_pos() {
            match token {
                Token::BraceClose 
                | Token::KwLet | Token::KwInt | Token::KwFloat | Token::KwBool 
                | Token::KwString | Token::KwQubit | Token::KwCbit | Token::KwQreg
                | Token::KwIf | Token::KwWhile | Token::KwFor | Token::KwBreak
                | Token::KwContinue | Token::KwReturn | Token::KwQIf | Token::KwQFor
                | Token::BraceOpen => break,
                _ => {
                    self.next_token();
                }
            }
        }
    }
    
    fn expect(&mut self, expected: &Token, context: &str) -> Option<()> {
        let peek_result = self.peek_token_with_pos().cloned();
        
        if let Some((token, line, col)) = peek_result {
            if token == *expected {
                self.next_token();
                Some(())
            } else {
                self.add_error(
                    format!("Expected '{}' {}, found '{}'", 
                           self.token_to_string(expected), 
                           context,
                           self.token_to_string(&token)),
                    line,
                    col,
                    Some(format!("Add '{}' here", self.token_to_string(expected))),
                );
                None
            }
        } else {
            self.add_error(
                format!("Expected '{}' {}, but reached end of file", 
                       self.token_to_string(expected), 
                       context),
                0,
                0,
                Some(format!("Add '{}' here", self.token_to_string(expected))),
            );
            None
        }
    }
    
    fn expect_ident(&mut self, context: &str) -> Option<String> {
        if let Some((token, line, col)) = self.next_token() {
            match token {
                Token::Ident(name) => Some(name),
                _ => {
                    self.add_error(
                        format!("Expected identifier for {}, found '{}'", 
                               context,
                               self.token_to_string(&token)),
                        line,
                        col,
                        Some("Example: 'myVariable', 'q', 'result'".to_string()),
                    );
                    None
                }
            }
        } else {
            self.add_error(
                format!("Expected identifier for {}, but reached end of file", context),
                0,
                0,
                None,
            );
            None
        }
    }
    
    fn peek_token(&mut self) -> Option<&Token> {
        self.tokens.peek().map(|(token, _, _)| token)
    }
    
    fn peek_token_with_pos(&mut self) -> Option<&(Token, usize, usize)> {
        self.tokens.peek()
    }
    
    fn next_token(&mut self) -> Option<(Token, usize, usize)> {
        let (token, line, col) = self.tokens.next()?;
        self.position += 1;
        Some((token, line, col))
    }
    
    fn consume_if(&mut self, expected: &Token) -> bool {
        if self.peek_token() == Some(expected) {
            self.next_token();
            true
        } else {
            false
        }
    }
    
    fn add_span_to_stmt(&self, stmt: Stmt, span: Span) -> Stmt {
        match stmt {
            Stmt::Expr(expr, _) => Stmt::Expr(expr, span),
            Stmt::Let(name, ty, expr, mutable, _) => Stmt::Let(name, ty, expr, mutable, span),
            Stmt::Assign(name, expr, _) => Stmt::Assign(name, expr, span),
            Stmt::Block(stmts, _) => Stmt::Block(stmts, span),
            Stmt::If(cond, then_stmt, else_stmt, _) => Stmt::If(cond, then_stmt, else_stmt, span),
            Stmt::While(cond, body, _) => Stmt::While(cond, body, span),
            Stmt::ForRange(var, start, end, step, body, _) => Stmt::ForRange(var, start, end, step, body, span),
            Stmt::Return(expr, _) => Stmt::Return(expr, span),
            Stmt::Break(_) => Stmt::Break(span),
            Stmt::Continue(_) => Stmt::Continue(span),
            Stmt::QIf(cond, then_stmt, else_stmt, _) => Stmt::QIf(cond, then_stmt, else_stmt, span),
            Stmt::QForRange(var, start, end, step, body, _) => Stmt::QForRange(var, start, end, step, body, span),
            Stmt::TypeAlias(alias, _) => Stmt::TypeAlias(alias, span),
            Stmt::StructDef(struct_def, _) => Stmt::StructDef(struct_def, span),
        }
    }
    
    fn token_to_string(&self, token: &Token) -> String {
        match token {
            Token::KwInt => "int".to_string(),
            Token::KwFloat => "float".to_string(),
            Token::KwBool => "bool".to_string(),
            Token::KwString => "string".to_string(),
            Token::KwQubit => "qubit".to_string(),
            Token::KwCbit => "cbit".to_string(),
            Token::KwIf => "if".to_string(),
            Token::KwElse => "else".to_string(),
            Token::KwWhile => "while".to_string(),
            Token::KwFor => "for".to_string(),
            Token::KwBreak => "break".to_string(),
            Token::KwContinue => "continue".to_string(),
            Token::KwReturn => "return".to_string(),
            Token::KwFn => "fn".to_string(),
            Token::KwLet => "let".to_string(),
            Token::KwIn => "in".to_string(),
            Token::KwRange => "range".to_string(),
            Token::KwQIf => "qif".to_string(),
            Token::KwQElse => "qelse".to_string(),
            Token::KwQFor => "qfor".to_string(),
            Token::KwQreg => "qreg".to_string(),
            Token::KwMut => "mut".to_string(),
            Token::KwType => "type".to_string(),
            Token::KwStruct => "struct".to_string(),
            Token::KwTuple => "tuple".to_string(),
            Token::IntLiteral(n) => format!("integer {}", n),
            Token::FloatLiteral(f) => format!("float {}", f),
            Token::StringLiteral(s) => format!("string \"{}\"", s),
            Token::QubitLiteral(bits) => {
                let s: String = bits.bits.iter().map(|b| if *b == 0 { '0' } else { '1' }).collect();
                format!("qubit |{}>", s)
            },
            Token::Ident(name) => name.to_string(),
            Token::OpAssign => "=".to_string(),
            Token::OpEq => "==".to_string(),
            Token::OpNeq => "!=".to_string(),
            Token::OpLt => "<".to_string(),
            Token::OpGt => ">".to_string(),
            Token::OpLe => "<=".to_string(),
            Token::OpGe => ">=".to_string(),
            Token::OpAdd => "+".to_string(),
            Token::OpSub => "-".to_string(),
            Token::OpMul => "*".to_string(),
            Token::OpDiv => "/".to_string(),
            Token::OpAnd => "&".to_string(),
            Token::OpOr => "|".to_string(),
            Token::OpXor => "^".to_string(),
            Token::OpNot => "!".to_string(),
            Token::OpIncrement => "++".to_string(),
            Token::OpDecrement => "--".to_string(),
            Token::OpAddAssign => "+=".to_string(),
            Token::OpSubAssign => "-=".to_string(),
            Token::OpMulAssign => "*=".to_string(),
            Token::OpDivAssign => "/=".to_string(),
            Token::ParenOpen => "(".to_string(),
            Token::ParenClose => ")".to_string(),
            Token::BraceOpen => "{".to_string(),
            Token::BraceClose => "}".to_string(),
            Token::BracketOpen => "[".to_string(),
            Token::BracketClose => "]".to_string(),
            Token::Comma => ",".to_string(),
            Token::Colon => ":".to_string(),
            Token::Semicolon => ";".to_string(),
            Token::Arrow => "->".to_string(),
            Token::Dot => ".".to_string(),
            Token::__Skip => "<skip>".to_string(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "{}:{}: {}", self.line, self.column, self.message)?;
        } else {
            write!(f, "{}", self.message)?;
        }
        if let Some(hint) = &self.hint {
            write!(f, "\n  hint: {}", hint)?;
        }
        Ok(())
    }
}