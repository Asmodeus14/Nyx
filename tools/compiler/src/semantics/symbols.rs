// src/semantics/symbols.rs - FULLY CORRECTED
use std::collections::HashMap;
use crate::ast::{Type, StructDef, Param};

#[derive(Debug, Clone)]
pub struct TypeRegistry {
    pub type_aliases: HashMap<String, Type>,
    pub struct_defs: HashMap<String, StructDef>,
    pub builtin_types: HashMap<String, Type>,
}

impl TypeRegistry {
    pub fn new() -> Self {
        let mut builtin_types = HashMap::new();
        builtin_types.insert("int".to_string(), Type::Int);
        builtin_types.insert("float".to_string(), Type::Float);
        builtin_types.insert("bool".to_string(), Type::Bool);
        builtin_types.insert("string".to_string(), Type::String);
        builtin_types.insert("qubit".to_string(), Type::Qubit);
        builtin_types.insert("cbit".to_string(), Type::Cbit);
        builtin_types.insert("unit".to_string(), Type::Unit);
        
        Self {
            type_aliases: HashMap::new(),
            struct_defs: HashMap::new(),
            builtin_types,
        }
    }
    
    pub fn add_type_alias(&mut self, name: String, target: Type) {
        self.type_aliases.insert(name, target);
    }
    
    pub fn add_struct_def(&mut self, struct_def: StructDef) {
        self.struct_defs.insert(struct_def.name.clone(), struct_def);
    }
    
    pub fn resolve_type(&self, ty: &Type) -> Result<Type, String> {
        match ty {
            Type::Named(name) => {
                // Check built-in types first
                if let Some(builtin) = self.builtin_types.get(name) {
                    return Ok(builtin.clone());
                }
                
                // Check type aliases
                if let Some(aliased) = self.type_aliases.get(name) {
                    return self.resolve_type(aliased);
                }
                
                // Check struct definitions
                if self.struct_defs.contains_key(name) {
                    return Ok(Type::Named(name.clone()));
                }
                
                Err(format!("Unknown type: '{}'", name))
            }
            
            Type::Array(inner, size) => {
                let resolved_inner = self.resolve_type(inner)?;
                Ok(Type::Array(Box::new(resolved_inner), *size))
            }
            
            Type::Tuple(types) => {
                let mut resolved_types = Vec::new();
                for t in types {
                    resolved_types.push(self.resolve_type(t)?);
                }
                Ok(Type::Tuple(resolved_types))
            }
            
            Type::Function(params, return_type) => {
                let mut resolved_params = Vec::new();
                for param_ty in params {
                    resolved_params.push(self.resolve_type(param_ty)?);
                }
                let resolved_return = self.resolve_type(return_type)?;
                Ok(Type::Function(resolved_params, Box::new(resolved_return)))
            }
            
            Type::Qreg(size) => Ok(Type::Qreg(*size)),
            
            _ => Ok(ty.clone()),
        }
    }
    
    pub fn is_quantum_type(&self, ty: &Type) -> Result<bool, String> {
        let resolved = self.resolve_type(ty)?;
        Ok(match resolved {
            Type::Qubit | Type::Qreg(_) => true,
            Type::Named(name) => {
                if let Some(struct_def) = self.struct_defs.get(&name) {
                    // Check if struct contains any quantum types
                    for field in &struct_def.fields {
                        if self.is_quantum_type(&field.ty)? {
                            return Ok(true);
                        }
                    }
                }
                false
            }
            Type::Tuple(types) => {
                for t in types {
                    if self.is_quantum_type(&t)? {
                        return Ok(true);
                    }
                }
                false
            }
            Type::Array(inner, _) => self.is_quantum_type(&inner)?,
            _ => false,
        })
    }
    
    pub fn get_struct_def(&self, name: &str) -> Option<&StructDef> {
        self.struct_defs.get(name)
    }
}

#[derive(Debug, Clone)]
pub struct SymbolTable {
    scopes: Vec<HashMap<String, Symbol>>,
}

#[derive(Debug, Clone)]
pub enum Symbol {
    Variable {
        name: String,
        ty: Type,
        mutable: bool,
        defined: bool,
    },
    Function {
        name: String,
        params: Vec<Param>,
        return_type: Type,
        defined: bool,
    },
    TypeAlias {
        name: String,
        target: Type,
    },
    Struct {
        name: String,
        definition: StructDef,
    },
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }
    
    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    
    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }
    
    pub fn current_scope(&self) -> &HashMap<String, Symbol> {
        self.scopes.last().unwrap()
    }
    
    pub fn current_scope_mut(&mut self) -> &mut HashMap<String, Symbol> {
        self.scopes.last_mut().unwrap()
    }
    
    pub fn insert(&mut self, symbol: Symbol) -> Result<(), String> {
        let name = match &symbol {
            Symbol::Variable { name, .. } => name.clone(),
            Symbol::Function { name, .. } => name.clone(),
            Symbol::TypeAlias { name, .. } => name.clone(),
            Symbol::Struct { name, .. } => name.clone(),
        };
        
        if self.current_scope().contains_key(&name) {
            return Err(format!("Symbol '{}' already defined in this scope", name));
        }
        
        self.current_scope_mut().insert(name, symbol);
        Ok(())
    }
    
    pub fn lookup(&self, name: &str) -> Option<&Symbol> {
        for scope in self.scopes.iter().rev() {
            if let Some(symbol) = scope.get(name) {
                return Some(symbol);
            }
        }
        None
    }
    
    pub fn lookup_variable(&self, name: &str) -> Option<(&Type, bool, bool)> {
        if let Some(Symbol::Variable { ty, mutable, defined, .. }) = self.lookup(name) {
            Some((ty, *mutable, *defined))
        } else {
            None
        }
    }
    
    pub fn lookup_function(&self, name: &str) -> Option<(Vec<Param>, Type, bool)> {
        if let Some(Symbol::Function { params, return_type, defined, .. }) = self.lookup(name) {
            Some((params.clone(), return_type.clone(), *defined))
        } else {
            None
        }
    }
    
    pub fn mark_variable_defined(&mut self, name: &str) -> Result<(), String> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(Symbol::Variable { defined, .. }) = scope.get_mut(name) {
                *defined = true;
                return Ok(());
            }
        }
        Err(format!("Variable '{}' not found", name))
    }
    
    pub fn mark_function_defined(&mut self, name: &str) -> Result<(), String> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(Symbol::Function { defined, .. }) = scope.get_mut(name) {
                *defined = true;
                return Ok(());
            }
        }
        Err(format!("Function '{}' not found", name))
    }
    
    pub fn contains(&self, name: &str) -> bool {
        self.current_scope().contains_key(name)
    }
}