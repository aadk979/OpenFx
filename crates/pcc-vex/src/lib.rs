//! Fuel-metered, statically type-checked VEX-lite data-parallel point kernel compiler and VM.
//!
//! Supports `@P`, `@ptnum`, `@numpt`, `@Frame`, custom `f@`/`i@`/`v@` point attributes,
//! `.x`/`.y`/`.z` vector swizzles, local variables (`float`, `int`, `vec3`), `if`/`else` and
//! fuel-metered `while` control flow, and deterministic math/noise built-ins (`Philox4x32`).

use pcc_geo::{AttributeColumn, GeoDetail, GeoError, PAGE_LEN};
use pcc_math::{Philox4x32, Vec3};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicBool, Ordering},
};

pub const DEFAULT_MAX_FUEL_PER_POINT: u32 = 10_000;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum VexType {
    Float,
    Int,
    Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VexValue {
    Float(f32),
    Int(i32),
    Vec3(Vec3),
}

impl VexValue {
    fn as_f32(self) -> Option<f32> {
        match self {
            Self::Float(v) => Some(v),
            Self::Int(v) => Some(v as f32),
            Self::Vec3(_) => None,
        }
    }

    fn as_i32(self) -> Option<i32> {
        match self {
            Self::Int(v) => Some(v),
            Self::Float(v) => Some(v as i32),
            Self::Vec3(_) => None,
        }
    }

    fn as_vec3(self) -> Option<Vec3> {
        match self {
            Self::Vec3(v) => Some(v),
            Self::Float(_) | Self::Int(_) => None,
        }
    }

    fn is_truthy(self) -> bool {
        match self {
            Self::Float(v) => v != 0.0,
            Self::Int(v) => v != 0,
            Self::Vec3(v) => v.x != 0.0 || v.y != 0.0 || v.z != 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VexDiagnostic {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VexCheckReport {
    pub valid: bool,
    pub read_attributes: Vec<String>,
    pub written_attributes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<VexDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VexError {
    Syntax(VexDiagnostic),
    Type(VexDiagnostic),
    FuelExhausted { line: usize, column: usize },
    NonFiniteOutput { point_index: usize },
    Geo(GeoError),
}

impl VexError {
    #[must_use]
    pub fn diagnostic(&self) -> VexDiagnostic {
        match self {
            Self::Syntax(d) | Self::Type(d) => d.clone(),
            Self::FuelExhausted { line, column } => VexDiagnostic {
                line: *line,
                column: *column,
                message: "kernel execution exceeded per-point loop/instruction fuel budget"
                    .to_owned(),
            },
            Self::NonFiniteOutput { point_index } => VexDiagnostic {
                line: 1,
                column: 1,
                message: format!("kernel produced non-finite value at point {point_index}"),
            },
            Self::Geo(err) => VexDiagnostic {
                line: 1,
                column: 1,
                message: format!("geometry error during wrangle execution: {err:?}"),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Component {
    X,
    Y,
    Z,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnaryOp {
    Neg,
    Not,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TargetRef {
    Local {
        name: String,
        comp: Option<Component>,
    },
    Attr {
        name: String,
        ty: VexType,
        comp: Option<Component>,
    },
}

#[derive(Clone, Debug)]
enum Expr {
    FloatLit(f32),
    IntLit(i32),
    Local {
        name: String,
        comp: Option<Component>,
    },
    Attr {
        name: String,
        ty: VexType,
        comp: Option<Component>,
    },
    BuiltinVar(BuiltinVar),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Call {
        func: BuiltinFunc,
        args: Vec<Expr>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuiltinVar {
    PtNum,
    NumPt,
    Frame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuiltinFunc {
    Vec3,
    Float,
    Int,
    Sin,
    Cos,
    Abs,
    Sqrt,
    Floor,
    Ceil,
    Min,
    Max,
    Clamp,
    Lerp,
    Fit,
    Length,
    Dot,
    Normalize,
    Noise,
}

#[derive(Clone, Debug)]
enum Stmt {
    VarDecl {
        name: String,
        init: Expr,
        line: usize,
        col: usize,
    },
    Assign {
        target: TargetRef,
        op: AssignOp,
        expr: Expr,
        line: usize,
        col: usize,
    },
    If {
        cond: Expr,
        then_body: Vec<Stmt>,
        else_body: Vec<Stmt>,
        line: usize,
        col: usize,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        line: usize,
        col: usize,
    },
}

#[derive(Clone, Debug)]
pub struct VexProgram {
    stmts: Vec<Stmt>,
    read_attrs: BTreeMap<String, VexType>,
    written_attrs: BTreeMap<String, VexType>,
}

impl VexProgram {
    #[must_use]
    pub fn read_attributes(&self) -> Vec<String> {
        self.read_attrs.keys().cloned().collect()
    }

    #[must_use]
    pub fn written_attributes(&self) -> Vec<String> {
        self.written_attrs.keys().cloned().collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
enum TokenKind {
    KwFloat,
    KwInt,
    KwVec3,
    KwIf,
    KwElse,
    KwWhile,
    Ident(String),
    Attr { prefix: Option<char>, name: String },
    Float(f32),
    Int(i32),
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Semi,
    Dot,
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    Plus,
    Minus,
    Star,
    Slash,
    Bang,
    Lt,
    Le,
    Gt,
    Ge,
    EqEq,
    BangEq,
    Eof,
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
    line: usize,
    col: usize,
}

fn default_attr_type(prefix: Option<char>, name: &str) -> Result<VexType, String> {
    match prefix {
        Some('f') => Ok(VexType::Float),
        Some('i') => Ok(VexType::Int),
        Some('v') => Ok(VexType::Vec3),
        Some(other) => Err(format!(
            "unsupported attribute type prefix `{other}@` on `{name}`"
        )),
        None => match name {
            "P" | "N" | "Cd" | "v" | "up" => Ok(VexType::Vec3),
            "ptnum" | "numpt" | "id" => Ok(VexType::Int),
            _ => Ok(VexType::Float),
        },
    }
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn tokenize(source: &str) -> Result<Vec<Token>, VexError> {
    let chars: Vec<char> = source.chars().collect();
    let mut idx = 0usize;
    let mut line = 1usize;
    let mut col = 1usize;
    let mut tokens = Vec::new();

    while idx < chars.len() {
        let ch = chars[idx];
        if ch == '\n' {
            idx += 1;
            line += 1;
            col = 1;
            continue;
        }
        if ch.is_ascii_whitespace() {
            idx += 1;
            col += 1;
            continue;
        }
        // Line comment `//`
        if ch == '/' && chars.get(idx + 1) == Some(&'/') {
            while idx < chars.len() && chars[idx] != '\n' {
                idx += 1;
            }
            continue;
        }
        let start_line = line;
        let start_col = col;

        // Typed attribute prefix `f@name`, `i@name`, `v@name`
        if ch.is_ascii_alphabetic() && chars.get(idx + 1) == Some(&'@') {
            let prefix = Some(ch);
            idx += 2;
            col += 2;
            let name_start = idx;
            while idx < chars.len() && is_ident_continue(chars[idx]) {
                idx += 1;
                col += 1;
            }
            if idx == name_start {
                return Err(VexError::Syntax(VexDiagnostic {
                    line: start_line,
                    column: start_col,
                    message: "expected attribute name after `@`".to_owned(),
                }));
            }
            let name: String = chars[name_start..idx].iter().collect();
            tokens.push(Token {
                kind: TokenKind::Attr { prefix, name },
                line: start_line,
                col: start_col,
            });
            continue;
        }

        // Untyped attribute `@name`
        if ch == '@' {
            idx += 1;
            col += 1;
            let name_start = idx;
            while idx < chars.len() && is_ident_continue(chars[idx]) {
                idx += 1;
                col += 1;
            }
            if idx == name_start {
                return Err(VexError::Syntax(VexDiagnostic {
                    line: start_line,
                    column: start_col,
                    message: "expected attribute name after `@`".to_owned(),
                }));
            }
            let name: String = chars[name_start..idx].iter().collect();
            tokens.push(Token {
                kind: TokenKind::Attr { prefix: None, name },
                line: start_line,
                col: start_col,
            });
            continue;
        }

        // Identifiers and keywords
        if is_ident_start(ch) {
            let start = idx;
            while idx < chars.len() && is_ident_continue(chars[idx]) {
                idx += 1;
                col += 1;
            }
            let word: String = chars[start..idx].iter().collect();
            let kind = match word.as_str() {
                "float" => TokenKind::KwFloat,
                "int" => TokenKind::KwInt,
                "vec3" | "vector" => TokenKind::KwVec3,
                "if" => TokenKind::KwIf,
                "else" => TokenKind::KwElse,
                "while" => TokenKind::KwWhile,
                _ => TokenKind::Ident(word),
            };
            tokens.push(Token {
                kind,
                line: start_line,
                col: start_col,
            });
            continue;
        }

        // Numeric literals
        if ch.is_ascii_digit() {
            let start = idx;
            let mut is_float = false;
            while idx < chars.len() && chars[idx].is_ascii_digit() {
                idx += 1;
                col += 1;
            }
            if idx < chars.len() && chars[idx] == '.' {
                is_float = true;
                idx += 1;
                col += 1;
                while idx < chars.len() && chars[idx].is_ascii_digit() {
                    idx += 1;
                    col += 1;
                }
            }
            let text: String = chars[start..idx].iter().collect();
            if is_float {
                let val = text.parse::<f32>().map_err(|_| {
                    VexError::Syntax(VexDiagnostic {
                        line: start_line,
                        column: start_col,
                        message: format!("invalid float literal `{text}`"),
                    })
                })?;
                if !val.is_finite() {
                    return Err(VexError::Syntax(VexDiagnostic {
                        line: start_line,
                        column: start_col,
                        message: format!("non-finite float literal `{text}`"),
                    }));
                }
                tokens.push(Token {
                    kind: TokenKind::Float(val),
                    line: start_line,
                    col: start_col,
                });
            } else {
                let val = text.parse::<i32>().map_err(|_| {
                    VexError::Syntax(VexDiagnostic {
                        line: start_line,
                        column: start_col,
                        message: format!("invalid integer literal `{text}`"),
                    })
                })?;
                tokens.push(Token {
                    kind: TokenKind::Int(val),
                    line: start_line,
                    col: start_col,
                });
            }
            continue;
        }

        // Two-character operators
        let next_ch = chars.get(idx + 1).copied();
        let two_char = match (ch, next_ch) {
            ('+', Some('=')) => Some(TokenKind::AddAssign),
            ('-', Some('=')) => Some(TokenKind::SubAssign),
            ('*', Some('=')) => Some(TokenKind::MulAssign),
            ('<', Some('=')) => Some(TokenKind::Le),
            ('>', Some('=')) => Some(TokenKind::Ge),
            ('=', Some('=')) => Some(TokenKind::EqEq),
            ('!', Some('=')) => Some(TokenKind::BangEq),
            _ => None,
        };
        if let Some(kind) = two_char {
            idx += 2;
            col += 2;
            tokens.push(Token {
                kind,
                line: start_line,
                col: start_col,
            });
            continue;
        }

        let single = match ch {
            '(' => Some(TokenKind::LParen),
            ')' => Some(TokenKind::RParen),
            '{' => Some(TokenKind::LBrace),
            '}' => Some(TokenKind::RBrace),
            ',' => Some(TokenKind::Comma),
            ';' => Some(TokenKind::Semi),
            '.' => Some(TokenKind::Dot),
            '=' => Some(TokenKind::Assign),
            '+' => Some(TokenKind::Plus),
            '-' => Some(TokenKind::Minus),
            '*' => Some(TokenKind::Star),
            '/' => Some(TokenKind::Slash),
            '!' => Some(TokenKind::Bang),
            '<' => Some(TokenKind::Lt),
            '>' => Some(TokenKind::Gt),
            _ => None,
        };
        if let Some(kind) = single {
            idx += 1;
            col += 1;
            tokens.push(Token {
                kind,
                line: start_line,
                col: start_col,
            });
            continue;
        }

        return Err(VexError::Syntax(VexDiagnostic {
            line: start_line,
            column: start_col,
            message: format!("unexpected character `{ch}`"),
        }));
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        line,
        col,
    });
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    locals: BTreeMap<String, VexType>,
    read_attrs: BTreeMap<String, VexType>,
    written_attrs: BTreeMap<String, VexType>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            locals: BTreeMap::new(),
            read_attrs: BTreeMap::new(),
            written_attrs: BTreeMap::new(),
        }
    }

    fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .unwrap_or(&self.tokens[self.tokens.len() - 1])
    }

    fn advance(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn expect_token(&mut self, expected: &TokenKind, desc: &str) -> Result<Token, VexError> {
        let tok = self.peek().clone();
        if &tok.kind == expected {
            Ok(self.advance())
        } else {
            Err(VexError::Syntax(VexDiagnostic {
                line: tok.line,
                column: tok.col,
                message: format!("expected {desc}"),
            }))
        }
    }

    fn register_attr(
        map: &mut BTreeMap<String, VexType>,
        all_other: &BTreeMap<String, VexType>,
        name: &str,
        ty: VexType,
        line: usize,
        col: usize,
    ) -> Result<(), VexError> {
        if let Some(&existing) = map.get(name).or_else(|| all_other.get(name))
            && existing != ty
        {
            return Err(VexError::Type(VexDiagnostic {
                line,
                column: col,
                message: format!(
                    "attribute `@{name}` referenced with conflicting types ({existing:?} vs {ty:?})"
                ),
            }));
        }
        map.insert(name.to_owned(), ty);
        Ok(())
    }

    fn parse_component(&mut self) -> Result<Option<Component>, VexError> {
        if self.peek().kind != TokenKind::Dot {
            return Ok(None);
        }
        self.advance();
        let tok = self.advance();
        if let TokenKind::Ident(comp) = tok.kind {
            match comp.as_str() {
                "x" | "r" => Ok(Some(Component::X)),
                "y" | "g" => Ok(Some(Component::Y)),
                "z" | "b" => Ok(Some(Component::Z)),
                _ => Err(VexError::Type(VexDiagnostic {
                    line: tok.line,
                    column: tok.col,
                    message: format!("unknown vector component `.{comp}` (expected x, y, or z)"),
                })),
            }
        } else {
            Err(VexError::Syntax(VexDiagnostic {
                line: tok.line,
                column: tok.col,
                message: "expected component name (`x`, `y`, or `z`) after `.`".to_owned(),
            }))
        }
    }

    fn parse_program(&mut self) -> Result<Vec<Stmt>, VexError> {
        let mut stmts = Vec::new();
        while self.peek().kind != TokenKind::Eof {
            stmts.push(self.parse_stmt()?);
        }
        if stmts.is_empty() {
            return Err(VexError::Syntax(VexDiagnostic {
                line: 1,
                column: 1,
                message: "wrangle snippet cannot be empty".to_owned(),
            }));
        }
        Ok(stmts)
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, VexError> {
        self.expect_token(&TokenKind::LBrace, "`{`")?;
        let mut stmts = Vec::new();
        while self.peek().kind != TokenKind::RBrace && self.peek().kind != TokenKind::Eof {
            stmts.push(self.parse_stmt()?);
        }
        self.expect_token(&TokenKind::RBrace, "`}`")?;
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, VexError> {
        let tok = self.peek().clone();
        match &tok.kind {
            TokenKind::KwFloat | TokenKind::KwInt | TokenKind::KwVec3 => {
                // Could be a variable declaration `float x = ...;`
                let decl_ty = match tok.kind {
                    TokenKind::KwFloat => VexType::Float,
                    TokenKind::KwInt => VexType::Int,
                    _ => VexType::Vec3,
                };
                self.advance();
                let name_tok = self.advance();
                let TokenKind::Ident(name) = name_tok.kind else {
                    return Err(VexError::Syntax(VexDiagnostic {
                        line: name_tok.line,
                        column: name_tok.col,
                        message: "expected variable identifier after type keyword".to_owned(),
                    }));
                };
                self.expect_token(&TokenKind::Assign, "`=`")?;
                let (init, init_ty) = self.parse_expr()?;
                let coerced_init = Self::coerce_expr(init, init_ty, decl_ty, tok.line, tok.col)?;
                self.expect_token(&TokenKind::Semi, "`;`")?;
                self.locals.insert(name.clone(), decl_ty);
                Ok(Stmt::VarDecl {
                    name,
                    init: coerced_init,
                    line: tok.line,
                    col: tok.col,
                })
            }
            TokenKind::KwIf => {
                self.advance();
                self.expect_token(&TokenKind::LParen, "`(`")?;
                let (cond, _) = self.parse_expr()?;
                self.expect_token(&TokenKind::RParen, "`)`")?;
                let then_body = self.parse_block()?;
                let else_body = if self.peek().kind == TokenKind::KwElse {
                    self.advance();
                    self.parse_block()?
                } else {
                    Vec::new()
                };
                Ok(Stmt::If {
                    cond,
                    then_body,
                    else_body,
                    line: tok.line,
                    col: tok.col,
                })
            }
            TokenKind::KwWhile => {
                self.advance();
                self.expect_token(&TokenKind::LParen, "`(`")?;
                let (cond, _) = self.parse_expr()?;
                self.expect_token(&TokenKind::RParen, "`)`")?;
                let body = self.parse_block()?;
                Ok(Stmt::While {
                    cond,
                    body,
                    line: tok.line,
                    col: tok.col,
                })
            }
            TokenKind::Ident(_) | TokenKind::Attr { .. } => {
                let first = self.advance();
                let comp = self.parse_component()?;
                let op_tok = self.advance();
                let op = match op_tok.kind {
                    TokenKind::Assign => AssignOp::Assign,
                    TokenKind::AddAssign => AssignOp::AddAssign,
                    TokenKind::SubAssign => AssignOp::SubAssign,
                    TokenKind::MulAssign => AssignOp::MulAssign,
                    _ => {
                        return Err(VexError::Syntax(VexDiagnostic {
                            line: op_tok.line,
                            column: op_tok.col,
                            message: "expected assignment operator (`=`, `+=`, `-=`, `*=`)"
                                .to_owned(),
                        }));
                    }
                };
                let (rhs, rhs_ty) = self.parse_expr()?;
                self.expect_token(&TokenKind::Semi, "`;`")?;

                match first.kind {
                    TokenKind::Ident(name) => {
                        let Some(&var_ty) = self.locals.get(&name) else {
                            return Err(VexError::Type(VexDiagnostic {
                                line: first.line,
                                column: first.col,
                                message: format!("undefined local variable `{name}`"),
                            }));
                        };
                        let expected_rhs_ty = if comp.is_some() {
                            if var_ty != VexType::Vec3 {
                                return Err(VexError::Type(VexDiagnostic {
                                    line: first.line,
                                    column: first.col,
                                    message: format!(
                                        "cannot access vector component on non-vec3 variable `{name}`"
                                    ),
                                }));
                            }
                            VexType::Float
                        } else {
                            var_ty
                        };
                        let coerced_rhs = if op == AssignOp::MulAssign
                            && expected_rhs_ty == VexType::Vec3
                            && rhs_ty == VexType::Float
                        {
                            rhs
                        } else {
                            Self::coerce_expr(rhs, rhs_ty, expected_rhs_ty, first.line, first.col)?
                        };
                        Ok(Stmt::Assign {
                            target: TargetRef::Local { name, comp },
                            op,
                            expr: coerced_rhs,
                            line: first.line,
                            col: first.col,
                        })
                    }
                    TokenKind::Attr { prefix, name } => {
                        if matches!(name.as_str(), "ptnum" | "numpt" | "Frame") {
                            return Err(VexError::Type(VexDiagnostic {
                                line: first.line,
                                column: first.col,
                                message: format!("built-in `@{} ` is read-only", name),
                            }));
                        }
                        let attr_ty = default_attr_type(prefix, &name).map_err(|msg| {
                            VexError::Type(VexDiagnostic {
                                line: first.line,
                                column: first.col,
                                message: msg,
                            })
                        })?;
                        let expected_rhs_ty = if comp.is_some() {
                            if attr_ty != VexType::Vec3 {
                                return Err(VexError::Type(VexDiagnostic {
                                    line: first.line,
                                    column: first.col,
                                    message: format!(
                                        "cannot access vector component on non-vec3 attribute `@{name}`"
                                    ),
                                }));
                            }
                            VexType::Float
                        } else {
                            attr_ty
                        };
                        // Compound assignment or component write also reads existing attribute
                        if op != AssignOp::Assign || comp.is_some() {
                            Self::register_attr(
                                &mut self.read_attrs,
                                &self.written_attrs,
                                &name,
                                attr_ty,
                                first.line,
                                first.col,
                            )?;
                        }
                        Self::register_attr(
                            &mut self.written_attrs,
                            &self.read_attrs,
                            &name,
                            attr_ty,
                            first.line,
                            first.col,
                        )?;
                        let coerced_rhs = if op == AssignOp::MulAssign
                            && expected_rhs_ty == VexType::Vec3
                            && rhs_ty == VexType::Float
                        {
                            rhs
                        } else {
                            Self::coerce_expr(rhs, rhs_ty, expected_rhs_ty, first.line, first.col)?
                        };
                        Ok(Stmt::Assign {
                            target: TargetRef::Attr {
                                name,
                                ty: attr_ty,
                                comp,
                            },
                            op,
                            expr: coerced_rhs,
                            line: first.line,
                            col: first.col,
                        })
                    }
                    _ => Err(VexError::Syntax(VexDiagnostic {
                        line: first.line,
                        column: first.col,
                        message: "invalid assignment target".to_owned(),
                    })),
                }
            }
            _ => Err(VexError::Syntax(VexDiagnostic {
                line: tok.line,
                column: tok.col,
                message: "expected statement".to_owned(),
            })),
        }
    }

    fn coerce_expr(
        expr: Expr,
        actual: VexType,
        expected: VexType,
        line: usize,
        col: usize,
    ) -> Result<Expr, VexError> {
        if actual == expected {
            return Ok(expr);
        }
        if actual == VexType::Int && expected == VexType::Float {
            return Ok(Expr::Call {
                func: BuiltinFunc::Float,
                args: vec![expr],
            });
        }
        Err(VexError::Type(VexDiagnostic {
            line,
            column: col,
            message: format!("type mismatch: expected {expected:?}, found {actual:?}"),
        }))
    }

    fn parse_expr(&mut self) -> Result<(Expr, VexType), VexError> {
        self.parse_equality()
    }

    fn parse_equality(&mut self) -> Result<(Expr, VexType), VexError> {
        let (mut left, mut left_ty) = self.parse_comparison()?;
        while matches!(self.peek().kind, TokenKind::EqEq | TokenKind::BangEq) {
            let op_tok = self.advance();
            let op = if op_tok.kind == TokenKind::EqEq {
                BinOp::Eq
            } else {
                BinOp::Ne
            };
            let (right, right_ty) = self.parse_comparison()?;
            let (l, r) =
                Self::unify_numeric(left, left_ty, right, right_ty, op_tok.line, op_tok.col)?;
            left = Expr::Binary {
                op,
                left: Box::new(l),
                right: Box::new(r),
            };
            left_ty = VexType::Int;
        }
        Ok((left, left_ty))
    }

    fn parse_comparison(&mut self) -> Result<(Expr, VexType), VexError> {
        let (mut left, mut left_ty) = self.parse_additive()?;
        while matches!(
            self.peek().kind,
            TokenKind::Lt | TokenKind::Le | TokenKind::Gt | TokenKind::Ge
        ) {
            let op_tok = self.advance();
            let op = match op_tok.kind {
                TokenKind::Lt => BinOp::Lt,
                TokenKind::Le => BinOp::Le,
                TokenKind::Gt => BinOp::Gt,
                _ => BinOp::Ge,
            };
            let (right, right_ty) = self.parse_additive()?;
            let (l, r) =
                Self::unify_numeric(left, left_ty, right, right_ty, op_tok.line, op_tok.col)?;
            left = Expr::Binary {
                op,
                left: Box::new(l),
                right: Box::new(r),
            };
            left_ty = VexType::Int;
        }
        Ok((left, left_ty))
    }

    fn parse_additive(&mut self) -> Result<(Expr, VexType), VexError> {
        let (mut left, mut left_ty) = self.parse_multiplicative()?;
        while matches!(self.peek().kind, TokenKind::Plus | TokenKind::Minus) {
            let op_tok = self.advance();
            let op = if op_tok.kind == TokenKind::Plus {
                BinOp::Add
            } else {
                BinOp::Sub
            };
            let (right, right_ty) = self.parse_multiplicative()?;
            if left_ty == VexType::Vec3 && right_ty == VexType::Vec3 {
                left = Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
                left_ty = VexType::Vec3;
            } else {
                let (l, r, res_ty) =
                    Self::unify_arith(left, left_ty, right, right_ty, op_tok.line, op_tok.col)?;
                left = Expr::Binary {
                    op,
                    left: Box::new(l),
                    right: Box::new(r),
                };
                left_ty = res_ty;
            }
        }
        Ok((left, left_ty))
    }

    fn parse_multiplicative(&mut self) -> Result<(Expr, VexType), VexError> {
        let (mut left, mut left_ty) = self.parse_unary()?;
        while matches!(self.peek().kind, TokenKind::Star | TokenKind::Slash) {
            let op_tok = self.advance();
            let op = if op_tok.kind == TokenKind::Star {
                BinOp::Mul
            } else {
                BinOp::Div
            };
            let (right, right_ty) = self.parse_unary()?;
            if op == BinOp::Mul && left_ty == VexType::Vec3 && right_ty != VexType::Vec3 {
                let r =
                    Self::coerce_expr(right, right_ty, VexType::Float, op_tok.line, op_tok.col)?;
                left = Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(r),
                };
                left_ty = VexType::Vec3;
            } else if op == BinOp::Mul && left_ty != VexType::Vec3 && right_ty == VexType::Vec3 {
                let l = Self::coerce_expr(left, left_ty, VexType::Float, op_tok.line, op_tok.col)?;
                left = Expr::Binary {
                    op,
                    left: Box::new(l),
                    right: Box::new(right),
                };
                left_ty = VexType::Vec3;
            } else {
                let (l, r, res_ty) =
                    Self::unify_arith(left, left_ty, right, right_ty, op_tok.line, op_tok.col)?;
                left = Expr::Binary {
                    op,
                    left: Box::new(l),
                    right: Box::new(r),
                };
                left_ty = res_ty;
            }
        }
        Ok((left, left_ty))
    }

    fn parse_unary(&mut self) -> Result<(Expr, VexType), VexError> {
        let tok = self.peek().clone();
        if tok.kind == TokenKind::Minus {
            self.advance();
            let (inner, ty) = self.parse_unary()?;
            return Ok((
                Expr::Unary {
                    op: UnaryOp::Neg,
                    expr: Box::new(inner),
                },
                ty,
            ));
        }
        if tok.kind == TokenKind::Bang {
            self.advance();
            let (inner, _) = self.parse_unary()?;
            return Ok((
                Expr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(inner),
                },
                VexType::Int,
            ));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<(Expr, VexType), VexError> {
        let tok = self.advance();
        match tok.kind {
            TokenKind::Float(v) => Ok((Expr::FloatLit(v), VexType::Float)),
            TokenKind::Int(v) => Ok((Expr::IntLit(v), VexType::Int)),
            TokenKind::LParen => {
                let (expr, ty) = self.parse_expr()?;
                self.expect_token(&TokenKind::RParen, "`)`")?;
                Ok((expr, ty))
            }
            TokenKind::KwVec3 | TokenKind::KwFloat | TokenKind::KwInt => {
                let name = match tok.kind {
                    TokenKind::KwVec3 => "vec3",
                    TokenKind::KwFloat => "float",
                    _ => "int",
                };
                self.parse_call(name, tok.line, tok.col)
            }
            TokenKind::Ident(name) => {
                if self.peek().kind == TokenKind::LParen {
                    self.parse_call(&name, tok.line, tok.col)
                } else {
                    let comp = self.parse_component()?;
                    let Some(&var_ty) = self.locals.get(&name) else {
                        return Err(VexError::Type(VexDiagnostic {
                            line: tok.line,
                            column: tok.col,
                            message: format!("undefined identifier `{name}`"),
                        }));
                    };
                    let res_ty = if comp.is_some() {
                        if var_ty != VexType::Vec3 {
                            return Err(VexError::Type(VexDiagnostic {
                                line: tok.line,
                                column: tok.col,
                                message: format!(
                                    "cannot access component on non-vec3 variable `{name}`"
                                ),
                            }));
                        }
                        VexType::Float
                    } else {
                        var_ty
                    };
                    Ok((Expr::Local { name, comp }, res_ty))
                }
            }
            TokenKind::Attr { prefix, name } => {
                let comp = self.parse_component()?;
                match name.as_str() {
                    "ptnum" => {
                        if comp.is_some() || prefix.is_some_and(|p| p != 'i') {
                            return Err(VexError::Type(VexDiagnostic {
                                line: tok.line,
                                column: tok.col,
                                message: "`@ptnum` is an `int` scalar".to_owned(),
                            }));
                        }
                        return Ok((Expr::BuiltinVar(BuiltinVar::PtNum), VexType::Int));
                    }
                    "numpt" => {
                        if comp.is_some() || prefix.is_some_and(|p| p != 'i') {
                            return Err(VexError::Type(VexDiagnostic {
                                line: tok.line,
                                column: tok.col,
                                message: "`@numpt` is an `int` scalar".to_owned(),
                            }));
                        }
                        return Ok((Expr::BuiltinVar(BuiltinVar::NumPt), VexType::Int));
                    }
                    "Frame" => {
                        if comp.is_some() || prefix.is_some_and(|p| p != 'f') {
                            return Err(VexError::Type(VexDiagnostic {
                                line: tok.line,
                                column: tok.col,
                                message: "`@Frame` is a `float` scalar".to_owned(),
                            }));
                        }
                        return Ok((Expr::BuiltinVar(BuiltinVar::Frame), VexType::Float));
                    }
                    _ => {}
                }
                let attr_ty = default_attr_type(prefix, &name).map_err(|msg| {
                    VexError::Type(VexDiagnostic {
                        line: tok.line,
                        column: tok.col,
                        message: msg,
                    })
                })?;
                let res_ty = if comp.is_some() {
                    if attr_ty != VexType::Vec3 {
                        return Err(VexError::Type(VexDiagnostic {
                            line: tok.line,
                            column: tok.col,
                            message: format!(
                                "cannot access component on non-vec3 attribute `@{name}`"
                            ),
                        }));
                    }
                    VexType::Float
                } else {
                    attr_ty
                };
                Self::register_attr(
                    &mut self.read_attrs,
                    &self.written_attrs,
                    &name,
                    attr_ty,
                    tok.line,
                    tok.col,
                )?;
                Ok((
                    Expr::Attr {
                        name,
                        ty: attr_ty,
                        comp,
                    },
                    res_ty,
                ))
            }
            _ => Err(VexError::Syntax(VexDiagnostic {
                line: tok.line,
                column: tok.col,
                message: "expected expression".to_owned(),
            })),
        }
    }

    fn parse_call(
        &mut self,
        name: &str,
        line: usize,
        col: usize,
    ) -> Result<(Expr, VexType), VexError> {
        self.expect_token(&TokenKind::LParen, "`(`")?;
        let mut raw_args = Vec::new();
        if self.peek().kind != TokenKind::RParen {
            loop {
                raw_args.push(self.parse_expr()?);
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect_token(&TokenKind::RParen, "`)`")?;

        let check_arity = |expected: usize| -> Result<(), VexError> {
            if raw_args.len() != expected {
                Err(VexError::Type(VexDiagnostic {
                    line,
                    column: col,
                    message: format!(
                        "function `{name}` expects {expected} arguments, found {}",
                        raw_args.len()
                    ),
                }))
            } else {
                Ok(())
            }
        };

        let coerce_all_float = |args: Vec<(Expr, VexType)>| -> Result<Vec<Expr>, VexError> {
            args.into_iter()
                .map(|(e, t)| Self::coerce_expr(e, t, VexType::Float, line, col))
                .collect()
        };

        match name {
            "vec3" => {
                check_arity(3)?;
                let args = coerce_all_float(raw_args)?;
                Ok((
                    Expr::Call {
                        func: BuiltinFunc::Vec3,
                        args,
                    },
                    VexType::Vec3,
                ))
            }
            "float" => {
                check_arity(1)?;
                let (e, _) = raw_args.remove(0);
                Ok((
                    Expr::Call {
                        func: BuiltinFunc::Float,
                        args: vec![e],
                    },
                    VexType::Float,
                ))
            }
            "int" => {
                check_arity(1)?;
                let (e, _) = raw_args.remove(0);
                Ok((
                    Expr::Call {
                        func: BuiltinFunc::Int,
                        args: vec![e],
                    },
                    VexType::Int,
                ))
            }
            "sin" | "cos" | "abs" | "sqrt" | "floor" | "ceil" => {
                check_arity(1)?;
                let args = coerce_all_float(raw_args)?;
                let func = match name {
                    "sin" => BuiltinFunc::Sin,
                    "cos" => BuiltinFunc::Cos,
                    "abs" => BuiltinFunc::Abs,
                    "sqrt" => BuiltinFunc::Sqrt,
                    "floor" => BuiltinFunc::Floor,
                    _ => BuiltinFunc::Ceil,
                };
                Ok((Expr::Call { func, args }, VexType::Float))
            }
            "min" | "max" => {
                check_arity(2)?;
                let args = coerce_all_float(raw_args)?;
                let func = if name == "min" {
                    BuiltinFunc::Min
                } else {
                    BuiltinFunc::Max
                };
                Ok((Expr::Call { func, args }, VexType::Float))
            }
            "clamp" | "lerp" => {
                check_arity(3)?;
                let args = coerce_all_float(raw_args)?;
                let func = if name == "clamp" {
                    BuiltinFunc::Clamp
                } else {
                    BuiltinFunc::Lerp
                };
                Ok((Expr::Call { func, args }, VexType::Float))
            }
            "fit" => {
                check_arity(5)?;
                let args = coerce_all_float(raw_args)?;
                Ok((
                    Expr::Call {
                        func: BuiltinFunc::Fit,
                        args,
                    },
                    VexType::Float,
                ))
            }
            "length" => {
                check_arity(1)?;
                let (e, t) = raw_args.remove(0);
                let arg = Self::coerce_expr(e, t, VexType::Vec3, line, col)?;
                Ok((
                    Expr::Call {
                        func: BuiltinFunc::Length,
                        args: vec![arg],
                    },
                    VexType::Float,
                ))
            }
            "normalize" => {
                check_arity(1)?;
                let (e, t) = raw_args.remove(0);
                let arg = Self::coerce_expr(e, t, VexType::Vec3, line, col)?;
                Ok((
                    Expr::Call {
                        func: BuiltinFunc::Normalize,
                        args: vec![arg],
                    },
                    VexType::Vec3,
                ))
            }
            "dot" => {
                check_arity(2)?;
                let (e1, t1) = raw_args.remove(0);
                let (e0, t0) = raw_args.remove(0);
                let a0 = Self::coerce_expr(e0, t0, VexType::Vec3, line, col)?;
                let a1 = Self::coerce_expr(e1, t1, VexType::Vec3, line, col)?;
                Ok((
                    Expr::Call {
                        func: BuiltinFunc::Dot,
                        args: vec![a0, a1],
                    },
                    VexType::Float,
                ))
            }
            "noise" => {
                check_arity(1)?;
                let (e, t) = raw_args.remove(0);
                let arg = Self::coerce_expr(e, t, VexType::Vec3, line, col)?;
                Ok((
                    Expr::Call {
                        func: BuiltinFunc::Noise,
                        args: vec![arg],
                    },
                    VexType::Float,
                ))
            }
            _ => Err(VexError::Type(VexDiagnostic {
                line,
                column: col,
                message: format!("unknown VEX built-in function `{name}`"),
            })),
        }
    }

    fn unify_numeric(
        left: Expr,
        left_ty: VexType,
        right: Expr,
        right_ty: VexType,
        line: usize,
        col: usize,
    ) -> Result<(Expr, Expr), VexError> {
        let (l, r, _) = Self::unify_arith(left, left_ty, right, right_ty, line, col)?;
        Ok((l, r))
    }

    fn unify_arith(
        left: Expr,
        left_ty: VexType,
        right: Expr,
        right_ty: VexType,
        line: usize,
        col: usize,
    ) -> Result<(Expr, Expr, VexType), VexError> {
        match (left_ty, right_ty) {
            (VexType::Int, VexType::Int) => Ok((left, right, VexType::Int)),
            (VexType::Float, VexType::Float) => Ok((left, right, VexType::Float)),
            (VexType::Int, VexType::Float) => Ok((
                Expr::Call {
                    func: BuiltinFunc::Float,
                    args: vec![left],
                },
                right,
                VexType::Float,
            )),
            (VexType::Float, VexType::Int) => Ok((
                left,
                Expr::Call {
                    func: BuiltinFunc::Float,
                    args: vec![right],
                },
                VexType::Float,
            )),
            _ => Err(VexError::Type(VexDiagnostic {
                line,
                column: col,
                message: format!("incompatible operand types: {left_ty:?} and {right_ty:?}"),
            })),
        }
    }
}

pub fn compile_vex(source: &str) -> Result<VexProgram, VexError> {
    let tokens = tokenize(source)?;
    let mut parser = Parser::new(tokens);
    let stmts = parser.parse_program()?;
    Ok(VexProgram {
        stmts,
        read_attrs: parser.read_attrs,
        written_attrs: parser.written_attrs,
    })
}

#[must_use]
pub fn check_vex(source: &str) -> VexCheckReport {
    match compile_vex(source) {
        Ok(prog) => VexCheckReport {
            valid: true,
            read_attributes: prog.read_attributes(),
            written_attributes: prog.written_attributes(),
            diagnostic: None,
        },
        Err(err) => VexCheckReport {
            valid: false,
            read_attributes: Vec::new(),
            written_attributes: Vec::new(),
            diagnostic: Some(err.diagnostic()),
        },
    }
}

fn get_component(v: Vec3, comp: Component) -> f32 {
    match comp {
        Component::X => v.x,
        Component::Y => v.y,
        Component::Z => v.z,
    }
}

fn set_component(mut v: Vec3, comp: Component, val: f32) -> Result<Vec3, ()> {
    match comp {
        Component::X => v.x = val,
        Component::Y => v.y = val,
        Component::Z => v.z = val,
    }
    Vec3::checked(v.x, v.y, v.z).map_err(|_| ())
}

fn sample_noise_3d(pos: Vec3) -> f32 {
    let rng = Philox4x32::new(0x5645_585F_4E4F_4953);
    let ix = pos.x.floor() as i32;
    let iy = pos.y.floor() as i32;
    let iz = pos.z.floor() as i32;
    let fx = pos.x - (ix as f32);
    let fy = pos.y - (iy as f32);
    let fz = pos.z - (iz as f32);
    let u = fx * fx * (3.0 - 2.0 * fx);
    let v = fy * fy * (3.0 - 2.0 * fy);
    let w = fz * fz * (3.0 - 2.0 * fz);

    let corner = |dx: i32, dy: i32, dz: i32| -> f32 {
        let cx = (ix.wrapping_add(dx)) as u32;
        let cy = (iy.wrapping_add(dy)) as u32;
        let cz = (iz.wrapping_add(dz)) as u32;
        rng.generate_unit_f32([cx, cy, cz, 0])[0]
    };

    let c000 = corner(0, 0, 0);
    let c100 = corner(1, 0, 0);
    let c010 = corner(0, 1, 0);
    let c110 = corner(1, 1, 0);
    let c001 = corner(0, 0, 1);
    let c101 = corner(1, 0, 1);
    let c011 = corner(0, 1, 1);
    let c111 = corner(1, 1, 1);

    let x00 = c000 + u * (c100 - c000);
    let x10 = c010 + u * (c110 - c010);
    let x01 = c001 + u * (c101 - c001);
    let x11 = c011 + u * (c111 - c011);
    let y0 = x00 + v * (x10 - x00);
    let y1 = x01 + v * (x11 - x01);
    y0 + w * (y1 - y0)
}

struct PointExecContext {
    ptnum: i32,
    numpt: i32,
    frame: f32,
    fuel: u32,
    locals: BTreeMap<String, VexValue>,
    attrs: BTreeMap<String, VexValue>,
}

impl PointExecContext {
    fn burn_fuel(&mut self, line: usize, col: usize) -> Result<(), VexError> {
        if self.fuel == 0 {
            return Err(VexError::FuelExhausted { line, column: col });
        }
        self.fuel -= 1;
        Ok(())
    }

    fn eval_expr(&mut self, expr: &Expr, line: usize, col: usize) -> Result<VexValue, VexError> {
        self.burn_fuel(line, col)?;
        match expr {
            Expr::FloatLit(v) => Ok(VexValue::Float(*v)),
            Expr::IntLit(v) => Ok(VexValue::Int(*v)),
            Expr::BuiltinVar(b) => match b {
                BuiltinVar::PtNum => Ok(VexValue::Int(self.ptnum)),
                BuiltinVar::NumPt => Ok(VexValue::Int(self.numpt)),
                BuiltinVar::Frame => Ok(VexValue::Float(self.frame)),
            },
            Expr::Local { name, comp } => {
                let val = self
                    .locals
                    .get(name)
                    .copied()
                    .unwrap_or(VexValue::Float(0.0));
                if let Some(c) = comp {
                    let v = val.as_vec3().unwrap_or(Vec3::ZERO);
                    Ok(VexValue::Float(get_component(v, *c)))
                } else {
                    Ok(val)
                }
            }
            Expr::Attr { name, ty, comp } => {
                let default_val = match ty {
                    VexType::Float => VexValue::Float(0.0),
                    VexType::Int => VexValue::Int(0),
                    VexType::Vec3 => VexValue::Vec3(Vec3::ZERO),
                };
                let val = self.attrs.get(name).copied().unwrap_or(default_val);
                if let Some(c) = comp {
                    let v = val.as_vec3().unwrap_or(Vec3::ZERO);
                    Ok(VexValue::Float(get_component(v, *c)))
                } else {
                    Ok(val)
                }
            }
            Expr::Unary { op, expr: inner } => {
                let val = self.eval_expr(inner, line, col)?;
                match op {
                    UnaryOp::Neg => match val {
                        VexValue::Float(v) => Ok(VexValue::Float(-v)),
                        VexValue::Int(v) => Ok(VexValue::Int(v.wrapping_neg())),
                        VexValue::Vec3(v) => {
                            Ok(VexValue::Vec3(Vec3::checked(-v.x, -v.y, -v.z).map_err(
                                |_| VexError::NonFiniteOutput {
                                    point_index: self.ptnum as usize,
                                },
                            )?))
                        }
                    },
                    UnaryOp::Not => Ok(VexValue::Int(i32::from(!val.is_truthy()))),
                }
            }
            Expr::Binary { op, left, right } => {
                let lv = self.eval_expr(left, line, col)?;
                let rv = self.eval_expr(right, line, col)?;
                self.eval_binary(*op, lv, rv)
            }
            Expr::Call { func, args } => {
                let mut evaluated = Vec::with_capacity(args.len());
                for a in args {
                    evaluated.push(self.eval_expr(a, line, col)?);
                }
                self.eval_call(*func, &evaluated)
            }
        }
    }

    fn eval_binary(&self, op: BinOp, lv: VexValue, rv: VexValue) -> Result<VexValue, VexError> {
        let pt_idx = self.ptnum as usize;
        let check_f32 = |v: f32| -> Result<f32, VexError> {
            if v.is_finite() {
                Ok(v)
            } else {
                Err(VexError::NonFiniteOutput {
                    point_index: pt_idx,
                })
            }
        };
        match (lv, rv) {
            (VexValue::Vec3(a), VexValue::Vec3(b)) => {
                match op {
                    BinOp::Add => a.add_checked(b).map(VexValue::Vec3).map_err(|_| {
                        VexError::NonFiniteOutput {
                            point_index: pt_idx,
                        }
                    }),
                    BinOp::Sub => a.sub_checked(b).map(VexValue::Vec3).map_err(|_| {
                        VexError::NonFiniteOutput {
                            point_index: pt_idx,
                        }
                    }),
                    _ => Err(VexError::NonFiniteOutput {
                        point_index: pt_idx,
                    }),
                }
            }
            (VexValue::Vec3(a), VexValue::Float(s)) | (VexValue::Float(s), VexValue::Vec3(a))
                if op == BinOp::Mul =>
            {
                a.scale_checked(s)
                    .map(VexValue::Vec3)
                    .map_err(|_| VexError::NonFiniteOutput {
                        point_index: pt_idx,
                    })
            }
            (VexValue::Int(a), VexValue::Int(b)) => match op {
                BinOp::Add => Ok(VexValue::Int(a.wrapping_add(b))),
                BinOp::Sub => Ok(VexValue::Int(a.wrapping_sub(b))),
                BinOp::Mul => Ok(VexValue::Int(a.wrapping_mul(b))),
                BinOp::Div => {
                    if b == 0 {
                        Err(VexError::NonFiniteOutput {
                            point_index: pt_idx,
                        })
                    } else {
                        Ok(VexValue::Int(a.wrapping_div(b)))
                    }
                }
                BinOp::Lt => Ok(VexValue::Int(i32::from(a < b))),
                BinOp::Le => Ok(VexValue::Int(i32::from(a <= b))),
                BinOp::Gt => Ok(VexValue::Int(i32::from(a > b))),
                BinOp::Ge => Ok(VexValue::Int(i32::from(a >= b))),
                BinOp::Eq => Ok(VexValue::Int(i32::from(a == b))),
                BinOp::Ne => Ok(VexValue::Int(i32::from(a != b))),
            },
            _ => {
                let a = lv.as_f32().unwrap_or(0.0);
                let b = rv.as_f32().unwrap_or(0.0);
                match op {
                    BinOp::Add => Ok(VexValue::Float(check_f32(a + b)?)),
                    BinOp::Sub => Ok(VexValue::Float(check_f32(a - b)?)),
                    BinOp::Mul => Ok(VexValue::Float(check_f32(a * b)?)),
                    BinOp::Div => Ok(VexValue::Float(check_f32(a / b)?)),
                    BinOp::Lt => Ok(VexValue::Int(i32::from(a < b))),
                    BinOp::Le => Ok(VexValue::Int(i32::from(a <= b))),
                    BinOp::Gt => Ok(VexValue::Int(i32::from(a > b))),
                    BinOp::Ge => Ok(VexValue::Int(i32::from(a >= b))),
                    BinOp::Eq => Ok(VexValue::Int(i32::from(a == b))),
                    BinOp::Ne => Ok(VexValue::Int(i32::from(a != b))),
                }
            }
        }
    }

    fn eval_call(&self, func: BuiltinFunc, args: &[VexValue]) -> Result<VexValue, VexError> {
        let pt_idx = self.ptnum as usize;
        let check_f32 = |v: f32| -> Result<f32, VexError> {
            if v.is_finite() {
                Ok(v)
            } else {
                Err(VexError::NonFiniteOutput {
                    point_index: pt_idx,
                })
            }
        };
        let f = |i: usize| {
            args.get(i)
                .copied()
                .and_then(VexValue::as_f32)
                .unwrap_or(0.0)
        };
        match func {
            BuiltinFunc::Vec3 => {
                Vec3::checked(f(0), f(1), f(2))
                    .map(VexValue::Vec3)
                    .map_err(|_| VexError::NonFiniteOutput {
                        point_index: pt_idx,
                    })
            }
            BuiltinFunc::Float => Ok(VexValue::Float(check_f32(f(0))?)),
            BuiltinFunc::Int => Ok(VexValue::Int(
                args.first()
                    .copied()
                    .and_then(VexValue::as_i32)
                    .unwrap_or(0),
            )),
            BuiltinFunc::Sin => Ok(VexValue::Float(check_f32(f(0).sin())?)),
            BuiltinFunc::Cos => Ok(VexValue::Float(check_f32(f(0).cos())?)),
            BuiltinFunc::Abs => Ok(VexValue::Float(check_f32(f(0).abs())?)),
            BuiltinFunc::Sqrt => Ok(VexValue::Float(check_f32(f(0).sqrt())?)),
            BuiltinFunc::Floor => Ok(VexValue::Float(check_f32(f(0).floor())?)),
            BuiltinFunc::Ceil => Ok(VexValue::Float(check_f32(f(0).ceil())?)),
            BuiltinFunc::Min => Ok(VexValue::Float(check_f32(f(0).min(f(1)))?)),
            BuiltinFunc::Max => Ok(VexValue::Float(check_f32(f(0).max(f(1)))?)),
            BuiltinFunc::Clamp => {
                let lo = f(1);
                let hi = f(2).max(lo);
                Ok(VexValue::Float(check_f32(f(0).clamp(lo, hi))?))
            }
            BuiltinFunc::Lerp => {
                let a = f(0);
                let b = f(1);
                let t = f(2);
                Ok(VexValue::Float(check_f32(a + t * (b - a))?))
            }
            BuiltinFunc::Fit => {
                let val = f(0);
                let omin = f(1);
                let omax = f(2);
                let nmin = f(3);
                let nmax = f(4);
                let denom = omax - omin;
                let t = if denom.abs() > 1e-12 {
                    ((val - omin) / denom).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                Ok(VexValue::Float(check_f32(nmin + t * (nmax - nmin))?))
            }
            BuiltinFunc::Length => {
                let v = args
                    .first()
                    .copied()
                    .and_then(VexValue::as_vec3)
                    .unwrap_or(Vec3::ZERO);
                Ok(VexValue::Float(check_f32(v.length())?))
            }
            BuiltinFunc::Normalize => {
                let v = args
                    .first()
                    .copied()
                    .and_then(VexValue::as_vec3)
                    .unwrap_or(Vec3::ZERO);
                Ok(VexValue::Vec3(v.normalized_or_up()))
            }
            BuiltinFunc::Dot => {
                let a = args
                    .first()
                    .copied()
                    .and_then(VexValue::as_vec3)
                    .unwrap_or(Vec3::ZERO);
                let b = args
                    .get(1)
                    .copied()
                    .and_then(VexValue::as_vec3)
                    .unwrap_or(Vec3::ZERO);
                Ok(VexValue::Float(check_f32(a.dot(b))?))
            }
            BuiltinFunc::Noise => {
                let v = args
                    .first()
                    .copied()
                    .and_then(VexValue::as_vec3)
                    .unwrap_or(Vec3::ZERO);
                Ok(VexValue::Float(check_f32(sample_noise_3d(v))?))
            }
        }
    }

    fn apply_assign(
        &self,
        current: VexValue,
        op: AssignOp,
        rhs: VexValue,
    ) -> Result<VexValue, VexError> {
        match op {
            AssignOp::Assign => Ok(rhs),
            AssignOp::AddAssign => self.eval_binary(BinOp::Add, current, rhs),
            AssignOp::SubAssign => self.eval_binary(BinOp::Sub, current, rhs),
            AssignOp::MulAssign => self.eval_binary(BinOp::Mul, current, rhs),
        }
    }

    fn exec_stmts(&mut self, stmts: &[Stmt]) -> Result<(), VexError> {
        for stmt in stmts {
            match stmt {
                Stmt::VarDecl {
                    name,
                    init,
                    line,
                    col,
                    ..
                } => {
                    let val = self.eval_expr(init, *line, *col)?;
                    self.locals.insert(name.clone(), val);
                }
                Stmt::Assign {
                    target,
                    op,
                    expr,
                    line,
                    col,
                } => {
                    let rhs = self.eval_expr(expr, *line, *col)?;
                    match target {
                        TargetRef::Local { name, comp } => {
                            let existing = self
                                .locals
                                .get(name)
                                .copied()
                                .unwrap_or(VexValue::Float(0.0));
                            let updated = if let Some(c) = comp {
                                let base_vec = existing.as_vec3().unwrap_or(Vec3::ZERO);
                                let old_c = VexValue::Float(get_component(base_vec, *c));
                                let new_c =
                                    self.apply_assign(old_c, *op, rhs)?.as_f32().unwrap_or(0.0);
                                let next_vec =
                                    set_component(base_vec, *c, new_c).map_err(|()| {
                                        VexError::NonFiniteOutput {
                                            point_index: self.ptnum as usize,
                                        }
                                    })?;
                                VexValue::Vec3(next_vec)
                            } else {
                                self.apply_assign(existing, *op, rhs)?
                            };
                            self.locals.insert(name.clone(), updated);
                        }
                        TargetRef::Attr { name, ty, comp } => {
                            let default_val = match ty {
                                VexType::Float => VexValue::Float(0.0),
                                VexType::Int => VexValue::Int(0),
                                VexType::Vec3 => VexValue::Vec3(Vec3::ZERO),
                            };
                            let existing = self.attrs.get(name).copied().unwrap_or(default_val);
                            let updated = if let Some(c) = comp {
                                let base_vec = existing.as_vec3().unwrap_or(Vec3::ZERO);
                                let old_c = VexValue::Float(get_component(base_vec, *c));
                                let new_c =
                                    self.apply_assign(old_c, *op, rhs)?.as_f32().unwrap_or(0.0);
                                let next_vec =
                                    set_component(base_vec, *c, new_c).map_err(|()| {
                                        VexError::NonFiniteOutput {
                                            point_index: self.ptnum as usize,
                                        }
                                    })?;
                                VexValue::Vec3(next_vec)
                            } else {
                                self.apply_assign(existing, *op, rhs)?
                            };
                            self.attrs.insert(name.clone(), updated);
                        }
                    }
                }
                Stmt::If {
                    cond,
                    then_body,
                    else_body,
                    line,
                    col,
                } => {
                    let c = self.eval_expr(cond, *line, *col)?;
                    if c.is_truthy() {
                        self.exec_stmts(then_body)?;
                    } else if !else_body.is_empty() {
                        self.exec_stmts(else_body)?;
                    }
                }
                Stmt::While {
                    cond,
                    body,
                    line,
                    col,
                } => loop {
                    self.burn_fuel(*line, *col)?;
                    let c = self.eval_expr(cond, *line, *col)?;
                    if !c.is_truthy() {
                        break;
                    }
                    self.exec_stmts(body)?;
                },
            }
        }
        Ok(())
    }
}

impl VexProgram {
    /// Executes the compiled point wrangle program across all points of `geo`,
    /// preserving Copy-on-Write page sharing for any attributes not in `written_attributes`.
    pub fn execute(
        &self,
        geo: &GeoDetail,
        frame: f32,
        max_fuel_per_point: u32,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<GeoDetail, VexError> {
        if !frame.is_finite() {
            return Err(VexError::Geo(GeoError::NonFinite));
        }
        let pt_count = geo.len();
        if pt_count == 0 || self.written_attrs.is_empty() {
            return Ok(geo.clone());
        }

        // Check memory budget for newly allocated written columns
        let mut bytes_needed = 0usize;
        for ty in self.written_attrs.values() {
            let elem_bytes = match ty {
                VexType::Float | VexType::Int => 4usize,
                VexType::Vec3 => 12usize,
            };
            let col_bytes = pt_count
                .checked_mul(elem_bytes)
                .ok_or(VexError::Geo(GeoError::LimitExceeded))?;
            bytes_needed = bytes_needed
                .checked_add(col_bytes)
                .ok_or(VexError::Geo(GeoError::LimitExceeded))?;
        }
        if bytes_needed > memory_limit {
            return Err(VexError::Geo(GeoError::LimitExceeded));
        }

        let mut all_referenced: BTreeSet<(&String, VexType)> = BTreeSet::new();
        for (k, &t) in &self.read_attrs {
            all_referenced.insert((k, t));
        }
        for (k, &t) in &self.written_attrs {
            all_referenced.insert((k, t));
        }

        // Prepare output buffers only for written attributes
        let mut out_positions: Option<Vec<Vec3>> = if self.written_attrs.contains_key("P") {
            Some(Vec::with_capacity(pt_count))
        } else {
            None
        };
        let mut out_f32: BTreeMap<String, Vec<f32>> = BTreeMap::new();
        let mut out_i32: BTreeMap<String, Vec<i32>> = BTreeMap::new();
        let mut out_vec3: BTreeMap<String, Vec<Vec3>> = BTreeMap::new();
        for (name, &ty) in &self.written_attrs {
            if name == "P" {
                continue;
            }
            match ty {
                VexType::Float => {
                    out_f32.insert(name.clone(), Vec::with_capacity(pt_count));
                }
                VexType::Int => {
                    out_i32.insert(name.clone(), Vec::with_capacity(pt_count));
                }
                VexType::Vec3 => {
                    out_vec3.insert(name.clone(), Vec::with_capacity(pt_count));
                }
            }
        }

        let numpt = i32::try_from(pt_count).unwrap_or(i32::MAX);
        for idx in 0..pt_count {
            if idx.is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                return Err(VexError::Geo(GeoError::Cancelled));
            }
            let mut attrs = BTreeMap::new();
            for &(name, ty) in &all_referenced {
                if name == "P" {
                    let p = geo.point(idx).unwrap_or(Vec3::ZERO);
                    attrs.insert(name.clone(), VexValue::Vec3(p));
                } else {
                    let val = match (ty, geo.attribute(name)) {
                        (VexType::Float, Some(AttributeColumn::F32(col))) => {
                            VexValue::Float(col.get(idx).map(|f| f.get()).unwrap_or(0.0))
                        }
                        (VexType::Int, Some(AttributeColumn::I32(col))) => {
                            VexValue::Int(col.get(idx).unwrap_or(0))
                        }
                        (VexType::Vec3, Some(AttributeColumn::Vec3(col))) => {
                            VexValue::Vec3(col.get(idx).unwrap_or(Vec3::ZERO))
                        }
                        (VexType::Float, _) => VexValue::Float(0.0),
                        (VexType::Int, _) => VexValue::Int(0),
                        (VexType::Vec3, _) => VexValue::Vec3(Vec3::ZERO),
                    };
                    attrs.insert(name.clone(), val);
                }
            }

            let mut ctx = PointExecContext {
                ptnum: i32::try_from(idx).unwrap_or(i32::MAX),
                numpt,
                frame,
                fuel: max_fuel_per_point,
                locals: BTreeMap::new(),
                attrs,
            };
            ctx.exec_stmts(&self.stmts)?;

            if let Some(ref mut p_vec) = out_positions {
                let p = ctx
                    .attrs
                    .get("P")
                    .copied()
                    .and_then(VexValue::as_vec3)
                    .unwrap_or(Vec3::ZERO);
                p_vec.push(p);
            }
            for (name, vec) in &mut out_f32 {
                let v = ctx
                    .attrs
                    .get(name)
                    .copied()
                    .and_then(VexValue::as_f32)
                    .unwrap_or(0.0);
                vec.push(v);
            }
            for (name, vec) in &mut out_i32 {
                let v = ctx
                    .attrs
                    .get(name)
                    .copied()
                    .and_then(VexValue::as_i32)
                    .unwrap_or(0);
                vec.push(v);
            }
            for (name, vec) in &mut out_vec3 {
                let v = ctx
                    .attrs
                    .get(name)
                    .copied()
                    .and_then(VexValue::as_vec3)
                    .unwrap_or(Vec3::ZERO);
                vec.push(v);
            }
        }

        let mut result = if let Some(new_pts) = out_positions {
            let mut next = GeoDetail::points(new_pts);
            if !geo.prim_runs().is_empty() {
                let mut vtx = Vec::with_capacity(geo.vertex_count());
                for i in 0..geo.vertex_count() {
                    if let Some(pt) = geo.vertex_point(i) {
                        vtx.push(pt);
                    }
                }
                next = next
                    .with_primitives(geo.prim_runs().to_vec(), vtx)
                    .map_err(VexError::Geo)?;
            }
            for attr_name in geo.attribute_names() {
                if !self.written_attrs.contains_key(attr_name)
                    && let Some(col) = geo.attribute(attr_name)
                {
                    next = next
                        .with_attribute(attr_name.to_owned(), col.clone())
                        .map_err(VexError::Geo)?;
                }
            }
            next
        } else {
            geo.clone()
        };

        for (name, vals) in out_f32 {
            let col = AttributeColumn::f32_checked(vals).map_err(VexError::Geo)?;
            result = result.with_attribute(name, col).map_err(VexError::Geo)?;
        }
        for (name, vals) in out_i32 {
            let col = AttributeColumn::i32_values(vals);
            result = result.with_attribute(name, col).map_err(VexError::Geo)?;
        }
        for (name, vals) in out_vec3 {
            let col = AttributeColumn::vec3_values(vals);
            result = result.with_attribute(name, col).map_err(VexError::Geo)?;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vex_wave_deformation_custom_attributes_and_cow_sharing() {
        let base = GeoDetail::points(vec![
            Vec3::checked(0.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(1.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(3.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
        ]);
        let cancelled = AtomicBool::new(false);

        // 1. Kernel that only writes `f@pscale` and `v@Cd` must preserve CoW position page sharing with `base`!
        let attr_only_src = r#"
            float u = float(@ptnum) / float(@numpt);
            f@pscale = clamp(u + 0.25, 0.1, 1.0);
            v@Cd = vec3(u, 1.0 - u, 0.5);
        "#;
        let prog = compile_vex(attr_only_src);
        assert!(prog.is_ok());
        let Ok(prog) = prog else {
            return;
        };
        assert_eq!(prog.read_attributes(), Vec::<String>::new());
        assert_eq!(
            prog.written_attributes(),
            vec!["Cd".to_owned(), "pscale".to_owned()]
        );

        let with_attrs = prog.execute(&base, 1.0, DEFAULT_MAX_FUEL_PER_POINT, 1 << 20, &cancelled);
        assert!(with_attrs.is_ok());
        let Ok(with_attrs) = with_attrs else {
            return;
        };
        assert!(base.page_shared_with(&with_attrs, 0));
        assert!(with_attrs.attribute("pscale").is_some());
        assert!(with_attrs.attribute("Cd").is_some());

        // 2. Kernel that deforms `@P.y` using `sin`, `noise`, and `@Frame`, while preserving `pscale` and `Cd` CoW pages!
        let wave_src = r#"
            float n = noise(@P);
            @P.y = sin(@P.x + @Frame * 0.1) * 2.0 + n;
        "#;
        let wave_prog = compile_vex(wave_src);
        assert!(wave_prog.is_ok());
        let Ok(wave_prog) = wave_prog else {
            return;
        };
        let deformed = wave_prog.execute(
            &with_attrs,
            10.0,
            DEFAULT_MAX_FUEL_PER_POINT,
            1 << 20,
            &cancelled,
        );
        assert!(deformed.is_ok());
        if let Ok(deformed) = deformed {
            assert!(with_attrs.attribute_page_shared_with(&deformed, "pscale", 0));
            assert!(with_attrs.attribute_page_shared_with(&deformed, "Cd", 0));
            assert!(!with_attrs.page_shared_with(&deformed, 0));
        }
    }

    #[test]
    fn vex_sandbox_rejects_infinite_loops_non_finite_and_type_errors() {
        let base = GeoDetail::points(vec![Vec3::ZERO]);
        let cancelled = AtomicBool::new(false);

        // 1. Infinite loop aborts deterministically with `FuelExhausted`
        let inf_prog = compile_vex("int k = 0; while (1) { k += 1; @P.x = float(k); }");
        assert!(inf_prog.is_ok());
        if let Ok(inf_prog) = inf_prog {
            let res = inf_prog.execute(&base, 0.0, 200, 1 << 20, &cancelled);
            assert!(matches!(res, Err(VexError::FuelExhausted { .. })));
        }

        // 2. Division by zero produces `NonFiniteOutput`
        let div_zero = compile_vex("float z = 0.0; @P.x = 1.0 / z;");
        assert!(div_zero.is_ok());
        if let Ok(div_zero) = div_zero {
            let res = div_zero.execute(&base, 0.0, 200, 1 << 20, &cancelled);
            assert_eq!(
                res.err(),
                Some(VexError::NonFiniteOutput { point_index: 0 })
            );
        }

        // 3. Static type error caught by `check_vex`
        let bad_report = check_vex("@P = 1.0;");
        assert!(!bad_report.valid);
        assert!(bad_report.diagnostic.is_some());
    }
}
