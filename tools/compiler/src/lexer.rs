// lexer.rs - COMPLETE FOR PHASE 1.3
use logos::Logos;
use crate::ast::{BitString, Span};

#[derive(Logos, Debug, PartialEq, Clone)]
pub enum Token {
    // Keywords
    #[token("int")]
    KwInt,
    #[token("float")]
    KwFloat,
    #[token("bool")]
    KwBool,
    #[token("string")]
    KwString,
    #[token("qubit")]
    KwQubit,
    #[token("qreg")]
    KwQreg,
    #[token("cbit")]
    KwCbit,
    
    // Control flow keywords
    #[token("if")]
    KwIf,
    #[token("else")]
    KwElse,
    #[token("while")]
    KwWhile,
    #[token("for")]
    KwFor,
    #[token("break")]
    KwBreak,
    #[token("continue")]
    KwContinue,
    #[token("return")]
    KwReturn,
    
    // Function and variable keywords
    #[token("fn")]
    KwFn,
    #[token("let")]
    KwLet,
    #[token("mut")]
    KwMut,
    #[token("in")]
    KwIn,

    // Quantum control flow keywords
    #[token("qif")]
    KwQIf,
    #[token("qelse")]
    KwQElse,
    #[token("qfor")]
    KwQFor,

    // Range keyword
    #[token("range")]
    KwRange,
    
    // Type system keywords
    #[token("type")]
    KwType,
    #[token("struct")]
    KwStruct,
    #[token("tuple")]
    KwTuple,

    // Literals
    #[regex(r"[0-9]+", |lex| lex.slice().parse().ok())]
    IntLiteral(i64),
    #[regex(r"[0-9]+\.[0-9]*", |lex| lex.slice().parse().ok())]
    FloatLiteral(f64),
    #[regex(r#""[^"]*""#, |lex| lex.slice()[1..lex.slice().len()-1].to_string())]
    StringLiteral(String),

    #[regex(r"\|[01]+>", |lex| {
        let s = lex.slice();
        let state_str = &s[1..s.len()-1];
        let bits: Vec<u8> = state_str.chars()
            .map(|c| if c == '0' { 0u8 } else { 1u8 })
            .collect();
        Some(BitString::new(bits, Span::default()))
    })]
    QubitLiteral(BitString),

    // Identifiers
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    Ident(String),

    // Operators
    #[token("=")]
    OpAssign,
    #[token("==")]
    OpEq,
    #[token("!=")]
    OpNeq,
    #[token("<")]
    OpLt,
    #[token(">")]
    OpGt,
    #[token("<=")]
    OpLe,
    #[token(">=")]
    OpGe,
    #[token("+")]
    OpAdd,
    #[token("-")]
    OpSub,
    #[token("*")]
    OpMul,
    #[token("/")]
    OpDiv,
    #[token("&")]
    OpAnd,
    #[token("|")]
    OpOr,
    #[token("^")]
    OpXor,
    #[token("!")]
    OpNot,
    #[token("++")]
    OpIncrement,
    #[token("--")]
    OpDecrement,
    #[token("+=")]
    OpAddAssign,
    #[token("-=")]
    OpSubAssign,
    #[token("*=")]
    OpMulAssign,
    #[token("/=")]
    OpDivAssign,

    // Delimiters
    #[token("(")]
    ParenOpen,
    #[token(")")]
    ParenClose,
    #[token("{")]
    BraceOpen,
    #[token("}")]
    BraceClose,
    #[token("[")]
    BracketOpen,
    #[token("]")]
    BracketClose,
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
    #[token(";")]
    Semicolon,
    #[token("->")]
    Arrow,
    #[token(".")]
    Dot,

    // Skip token
    #[regex(r"//[^\n]*", logos::skip)]
    #[regex(r"/\*[^*]*\*+(?:[^/*][^*]*\*+)*/", logos::skip)]
    #[regex(r"[ \t\n\r\f]+", logos::skip)]
    __Skip,
}

pub fn tokenize(source: &str) -> Vec<(Token, usize, usize)> {
    let mut tokens = Vec::new();
    let mut lexer = Token::lexer(source);
    
    while let Some(result) = lexer.next() {
        match result {
            Ok(token) => {
                if token != Token::__Skip {
                    let span = lexer.span();
                    let token_start = span.start;
                    
                    let lines_up_to_token: Vec<&str> = source[..token_start].lines().collect();
                    let current_line = lines_up_to_token.len();
                    
                    let current_line_start = source[..token_start].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
                    let current_column = token_start - current_line_start + 1;
                    
                    tokens.push((token, current_line, current_column));
                }
            }
            Err(_) => {
                let span = lexer.span();
                let slice = lexer.slice();
                
                let lines_up_to_error: Vec<&str> = source[..span.start].lines().collect();
                let error_line = lines_up_to_error.len();
                let line_start = source[..span.start].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
                let error_column = span.start - line_start + 1;
                
                eprintln!("Lexer error at line {} column {}: unexpected character '{}'", 
                         error_line, error_column, slice);
            }
        }
    }
    
    tokens
}

pub fn is_gate_name(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "h" | "x" | "y" | "z" | "cnot" | "rx" | "ry" | "rz" | "t" | "s" | "swap"
    )
}