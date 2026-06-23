// src/semantics/errors.rs - FIXED
use crate::ast::Span;

#[derive(Debug, Clone)]
pub struct SemanticError {
    pub message: String,
    pub span: Span,
    pub hint: Option<String>,
}

impl SemanticError {
    pub fn new(span: &Span, message: &str, hint: Option<&str>) -> Self {
        Self {
            message: message.to_string(),
            span: span.clone(),
            hint: hint.map(|s| s.to_string()),
        }
    }
    
    pub fn format_with_source(&self, source: &str) -> String {
        let lines: Vec<&str> = source.lines().collect();
        if self.span.line > 0 && self.span.line <= lines.len() {
            let line_content = lines[self.span.line - 1];
            let indicator = " ".repeat(self.span.column.saturating_sub(1)) + "^";
            
            let mut result = format!(
                "Semantic error at line {}:{}: {}\n  {}\n  {}\n",
                self.span.line, self.span.column,
                self.message, line_content, indicator
            );
            
            if let Some(hint) = &self.hint {
                result.push_str(&format!("  hint: {}\n", hint));
            }
            
            result
        } else {
            let mut result = format!("Semantic error: {}", self.message);
            if let Some(hint) = &self.hint {
                result.push_str(&format!("\n  hint: {}", hint));
            }
            result
        }
    }
}

impl std::fmt::Display for SemanticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Semantic error at line {}:{}: {}", 
               self.span.line, self.span.column, self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, "\n  hint: {}", hint)?;
        }
        Ok(())
    }
}