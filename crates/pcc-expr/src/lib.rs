//! Parameter expressions: small, deterministic, side-effect-free scalar formulas such as
//! `$F * 0.1` or `ch(4, "radius") * 2`.
//!
//! Source compiles to stack bytecode whose required depth is known at compile time, so evaluation
//! allocates nothing and cannot overflow. There are no loops or user functions, so every expression
//! terminates, and evaluating one costs at most a fixed number of steps per source byte. Trigonometry
//! takes radians, matching the VEX-style language rather than HScript.

use pcc_math::Philox4x32;
use std::fmt;

/// Longest accepted source, which bounds compile time and bytecode size.
pub const MAX_SOURCE_LEN: usize = 4096;
/// Deepest evaluation stack and parser nesting an expression may need.
const MAX_STACK: usize = 64;

/// A fault in an expression, with a byte offset into its source where that is meaningful.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileError {
    pub position: usize,
    pub message: String,
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at byte {}: {}", self.position, self.message)
    }
}

/// Why a valid expression could not produce a number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvalError {
    UnknownChannel(String),
    /// The expression asked about an input the node does not have.
    NoSuchInput(u32),
    /// The result was NaN or infinite, as from `1 / 0`.
    NonFinite,
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownChannel(name) => write!(f, "no such channel: {name}"),
            Self::NoSuchInput(index) => write!(f, "no input {index}"),
            Self::NonFinite => write!(f, "the result is not finite"),
        }
    }
}

/// A geometry statistic of one of a node's inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputStat {
    Points,
    Prims,
    BoundsMin(usize),
    BoundsMax(usize),
    BoundsSize(usize),
    BoundsCenter(usize),
}

/// What an expression can see while it evaluates.
pub trait Scope {
    /// The current frame, which may be fractional.
    fn frame(&self) -> f64;
    /// Frames per second.
    fn fps(&self) -> f64;
    /// A literal or already-resolved parameter, on another node or this one when `node` is `None`.
    /// Vector components are named like `size.x`.
    fn channel(&self, node: Option<u64>, name: &str) -> Option<f64>;
    /// A statistic of the geometry on input `input`, or `None` if there is no such input.
    fn input_stat(&self, input: u32, stat: InputStat) -> Option<f64>;
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Const(f64),
    Frame,
    Time,
    Fps,
    /// Index into `Expr::channels`.
    Channel(u32),
    InputStat(u32, InputStat),
    Neg,
    Not,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
    Call(Func),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Func {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sqrt,
    Abs,
    Floor,
    Ceil,
    Round,
    Frac,
    Exp,
    Log,
    Log10,
    Deg,
    Rad,
    Sign,
    Rand,
    Atan2,
    Min,
    Max,
    Pow,
    Fmod,
    Step,
    Clamp,
    Lerp,
    Smoothstep,
    If,
    Fit,
}

impl Func {
    fn lookup(name: &str) -> Option<(Self, usize)> {
        Some(match name {
            "sin" => (Self::Sin, 1),
            "cos" => (Self::Cos, 1),
            "tan" => (Self::Tan, 1),
            "asin" => (Self::Asin, 1),
            "acos" => (Self::Acos, 1),
            "atan" => (Self::Atan, 1),
            "sqrt" => (Self::Sqrt, 1),
            "abs" => (Self::Abs, 1),
            "floor" => (Self::Floor, 1),
            "ceil" => (Self::Ceil, 1),
            "round" => (Self::Round, 1),
            "frac" => (Self::Frac, 1),
            "exp" => (Self::Exp, 1),
            "log" => (Self::Log, 1),
            "log10" => (Self::Log10, 1),
            "deg" => (Self::Deg, 1),
            "rad" => (Self::Rad, 1),
            "sign" => (Self::Sign, 1),
            "rand" => (Self::Rand, 1),
            "atan2" => (Self::Atan2, 2),
            "min" => (Self::Min, 2),
            "max" => (Self::Max, 2),
            "pow" => (Self::Pow, 2),
            "fmod" => (Self::Fmod, 2),
            "step" => (Self::Step, 2),
            "clamp" => (Self::Clamp, 3),
            "lerp" => (Self::Lerp, 3),
            "smoothstep" => (Self::Smoothstep, 3),
            "if" => (Self::If, 3),
            "fit" => (Self::Fit, 5),
            _ => return None,
        })
    }

    const fn arity(self) -> usize {
        match self {
            Self::Sin
            | Self::Cos
            | Self::Tan
            | Self::Asin
            | Self::Acos
            | Self::Atan
            | Self::Sqrt
            | Self::Abs
            | Self::Floor
            | Self::Ceil
            | Self::Round
            | Self::Frac
            | Self::Exp
            | Self::Log
            | Self::Log10
            | Self::Deg
            | Self::Rad
            | Self::Sign
            | Self::Rand => 1,
            Self::Atan2 | Self::Min | Self::Max | Self::Pow | Self::Fmod | Self::Step => 2,
            Self::Clamp | Self::Lerp | Self::Smoothstep | Self::If => 3,
            Self::Fit => 5,
        }
    }

    fn apply(self, a: &[f64]) -> f64 {
        match self {
            Self::Sin => a[0].sin(),
            Self::Cos => a[0].cos(),
            Self::Tan => a[0].tan(),
            Self::Asin => a[0].asin(),
            Self::Acos => a[0].acos(),
            Self::Atan => a[0].atan(),
            Self::Sqrt => a[0].sqrt(),
            Self::Abs => a[0].abs(),
            Self::Floor => a[0].floor(),
            Self::Ceil => a[0].ceil(),
            Self::Round => a[0].round(),
            Self::Frac => a[0] - a[0].floor(),
            Self::Exp => a[0].exp(),
            Self::Log => a[0].ln(),
            Self::Log10 => a[0].log10(),
            Self::Deg => a[0].to_degrees(),
            Self::Rad => a[0].to_radians(),
            Self::Sign => {
                if a[0] > 0.0 {
                    1.0
                } else if a[0] < 0.0 {
                    -1.0
                } else {
                    0.0
                }
            }
            Self::Rand => {
                // Keyed by the value's exact bits, so that equal inputs repeat on every machine.
                let bits = a[0].to_bits();
                let rng = Philox4x32::new(0x7065_7870_7261_6e64);
                let unit = rng.generate_unit_f32([bits as u32, (bits >> 32) as u32, 0, 0]);
                f64::from(unit[0])
            }
            Self::Atan2 => a[0].atan2(a[1]),
            Self::Min => a[0].min(a[1]),
            Self::Max => a[0].max(a[1]),
            Self::Pow => a[0].powf(a[1]),
            Self::Fmod => a[0] % a[1],
            Self::Step => {
                if a[1] >= a[0] {
                    1.0
                } else {
                    0.0
                }
            }
            Self::Clamp => a[0].max(a[1]).min(a[2]),
            Self::Lerp => a[0] + (a[1] - a[0]) * a[2],
            Self::Smoothstep => {
                let t = ((a[2] - a[0]) / (a[1] - a[0])).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            }
            Self::If => {
                if a[0] != 0.0 {
                    a[1]
                } else {
                    a[2]
                }
            }
            Self::Fit => {
                // fit(value, old_min, old_max, new_min, new_max), clamped to the old range.
                let t = ((a[0] - a[1]) / (a[2] - a[1])).clamp(0.0, 1.0);
                a[3] + (a[4] - a[3]) * t
            }
        }
    }
}

/// A compiled expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    ops: Vec<Op>,
    channels: Vec<(Option<u64>, String)>,
    uses_input_stats: bool,
}

impl Expr {
    /// Compiles source text, checking every name and argument count.
    pub fn compile(source: &str) -> Result<Self, CompileError> {
        if source.len() > MAX_SOURCE_LEN {
            return Err(CompileError {
                position: MAX_SOURCE_LEN,
                message: "the expression is too long".to_owned(),
            });
        }
        let mut parser = Parser {
            source,
            bytes: source.as_bytes(),
            at: 0,
            nesting: 0,
            depth: 0,
            max_depth: 0,
            out: Self {
                ops: Vec::new(),
                channels: Vec::new(),
                uses_input_stats: false,
            },
        };
        parser.or_expression()?;
        parser.skip_space();
        if parser.at != parser.bytes.len() {
            return Err(parser.error("unexpected text after the expression"));
        }
        if parser.out.ops.is_empty() {
            return Err(parser.error("the expression is empty"));
        }
        Ok(parser.out)
    }

    /// Other nodes this expression reads, which are dependencies in the graph.
    pub fn referenced_nodes(&self) -> impl Iterator<Item = u64> + '_ {
        self.channels.iter().filter_map(|(node, _)| *node)
    }

    /// Whether the result can change with time without any channel changing.
    #[must_use]
    pub fn is_time_dependent(&self) -> bool {
        self.ops
            .iter()
            .any(|op| matches!(op, Op::Frame | Op::Time | Op::Fps))
    }

    /// Whether this expression reads geometry on the node's inputs.
    #[must_use]
    pub const fn reads_inputs(&self) -> bool {
        self.uses_input_stats
    }

    /// Evaluates to a finite number.
    pub fn eval(&self, scope: &dyn Scope) -> Result<f64, EvalError> {
        let mut stack = [0.0f64; MAX_STACK];
        let mut top = 0usize;
        for op in &self.ops {
            match op {
                Op::Const(value) => {
                    stack[top] = *value;
                    top += 1;
                }
                Op::Frame => {
                    stack[top] = scope.frame();
                    top += 1;
                }
                Op::Time => {
                    stack[top] = scope.frame() / scope.fps();
                    top += 1;
                }
                Op::Fps => {
                    stack[top] = scope.fps();
                    top += 1;
                }
                Op::Channel(index) => {
                    let (node, name) = &self.channels[*index as usize];
                    stack[top] = scope
                        .channel(*node, name)
                        .ok_or_else(|| EvalError::UnknownChannel(name.clone()))?;
                    top += 1;
                }
                Op::InputStat(input, stat) => {
                    stack[top] = scope
                        .input_stat(*input, *stat)
                        .ok_or(EvalError::NoSuchInput(*input))?;
                    top += 1;
                }
                Op::Neg => stack[top - 1] = -stack[top - 1],
                Op::Not => stack[top - 1] = truth(stack[top - 1] == 0.0),
                Op::Call(func) => {
                    let arity = func.arity();
                    let value = func.apply(&stack[top - arity..top]);
                    top -= arity;
                    stack[top] = value;
                    top += 1;
                }
                binary => {
                    let (a, b) = (stack[top - 2], stack[top - 1]);
                    top -= 1;
                    stack[top - 1] = match binary {
                        Op::Add => a + b,
                        Op::Sub => a - b,
                        Op::Mul => a * b,
                        Op::Div => a / b,
                        Op::Rem => a % b,
                        Op::Pow => a.powf(b),
                        Op::Lt => truth(a < b),
                        Op::Le => truth(a <= b),
                        Op::Gt => truth(a > b),
                        Op::Ge => truth(a >= b),
                        Op::Eq => truth(a == b),
                        Op::Ne => truth(a != b),
                        Op::And => truth(a != 0.0 && b != 0.0),
                        _ => truth(a != 0.0 || b != 0.0),
                    };
                }
            }
        }
        let result = stack[0];
        if result.is_finite() {
            // Normalize negative zero, so that equal results hash equal in cache keys.
            Ok(if result == 0.0 { 0.0 } else { result })
        } else {
            Err(EvalError::NonFinite)
        }
    }
}

const fn truth(value: bool) -> f64 {
    if value { 1.0 } else { 0.0 }
}

struct Parser<'a> {
    source: &'a str,
    bytes: &'a [u8],
    at: usize,
    nesting: usize,
    depth: usize,
    max_depth: usize,
    out: Expr,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> CompileError {
        CompileError {
            position: self.at,
            message: message.to_owned(),
        }
    }

    fn skip_space(&mut self) {
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn eat(&mut self, token: &str) -> bool {
        self.skip_space();
        if self.bytes[self.at..].starts_with(token.as_bytes()) {
            self.at += token.len();
            true
        } else {
            false
        }
    }

    /// Emits an op, tracking the stack depth it will need at runtime.
    fn emit(&mut self, op: Op, popped: usize, pushed: usize) -> Result<(), CompileError> {
        self.depth = self.depth - popped + pushed;
        self.max_depth = self.max_depth.max(self.depth);
        if self.max_depth > MAX_STACK {
            return Err(self.error("the expression is too deeply nested"));
        }
        self.out.ops.push(op);
        Ok(())
    }

    /// Guards recursion, so hostile input like a thousand parentheses cannot overflow the stack.
    fn nested(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<(), CompileError>,
    ) -> Result<(), CompileError> {
        self.nesting += 1;
        if self.nesting > MAX_STACK {
            return Err(self.error("the expression is too deeply nested"));
        }
        let result = parse(self);
        self.nesting -= 1;
        result
    }

    fn or_expression(&mut self) -> Result<(), CompileError> {
        self.and_expression()?;
        while self.eat("||") {
            self.and_expression()?;
            self.emit(Op::Or, 2, 1)?;
        }
        Ok(())
    }

    fn and_expression(&mut self) -> Result<(), CompileError> {
        self.comparison()?;
        while self.eat("&&") {
            self.comparison()?;
            self.emit(Op::And, 2, 1)?;
        }
        Ok(())
    }

    fn comparison(&mut self) -> Result<(), CompileError> {
        self.additive()?;
        loop {
            // Two-character operators first, so that `<=` is not read as `<` then `=`.
            let op = if self.eat("<=") {
                Op::Le
            } else if self.eat(">=") {
                Op::Ge
            } else if self.eat("==") {
                Op::Eq
            } else if self.eat("!=") {
                Op::Ne
            } else if self.eat("<") {
                Op::Lt
            } else if self.eat(">") {
                Op::Gt
            } else {
                return Ok(());
            };
            self.additive()?;
            self.emit(op, 2, 1)?;
        }
    }

    fn additive(&mut self) -> Result<(), CompileError> {
        self.multiplicative()?;
        loop {
            let op = if self.eat("+") {
                Op::Add
            } else if self.eat("-") {
                Op::Sub
            } else {
                return Ok(());
            };
            self.multiplicative()?;
            self.emit(op, 2, 1)?;
        }
    }

    fn multiplicative(&mut self) -> Result<(), CompileError> {
        self.unary()?;
        loop {
            let op = if self.eat("*") {
                Op::Mul
            } else if self.eat("/") {
                Op::Div
            } else if self.eat("%") {
                Op::Rem
            } else {
                return Ok(());
            };
            self.unary()?;
            self.emit(op, 2, 1)?;
        }
    }

    fn unary(&mut self) -> Result<(), CompileError> {
        if self.eat("-") {
            self.nested(Self::unary)?;
            return self.emit(Op::Neg, 1, 1);
        }
        if self.eat("+") {
            return self.nested(Self::unary);
        }
        if self.eat("!") {
            self.nested(Self::unary)?;
            return self.emit(Op::Not, 1, 1);
        }
        self.power()
    }

    /// `^` binds tighter than unary minus on its left and is right-associative: `-2^2^3` is
    /// `-(2^(2^3))`.
    fn power(&mut self) -> Result<(), CompileError> {
        self.primary()?;
        if self.eat("^") {
            self.nested(Self::unary)?;
            self.emit(Op::Pow, 2, 1)?;
        }
        Ok(())
    }

    fn primary(&mut self) -> Result<(), CompileError> {
        self.skip_space();
        match self.bytes.get(self.at) {
            None => Err(self.error("the expression ends where a value was expected")),
            Some(b'(') => {
                self.at += 1;
                self.nested(Self::or_expression)?;
                if self.eat(")") {
                    Ok(())
                } else {
                    Err(self.error("expected `)`"))
                }
            }
            Some(b'$') => self.variable(),
            Some(c) if c.is_ascii_digit() || *c == b'.' => self.number(),
            Some(c) if c.is_ascii_alphabetic() => self.call(),
            Some(_) => Err(self.error("expected a value")),
        }
    }

    fn word(&mut self) -> &str {
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            self.at += 1;
        }
        &self.source[start..self.at]
    }

    fn variable(&mut self) -> Result<(), CompileError> {
        self.at += 1;
        let op = match self.word() {
            "F" => Op::Frame,
            "T" => Op::Time,
            "FPS" => Op::Fps,
            "PI" => Op::Const(std::f64::consts::PI),
            "E" => Op::Const(std::f64::consts::E),
            _ => return Err(self.error("unknown variable: use $F, $T, $FPS, $PI, or $E")),
        };
        self.emit(op, 0, 1)
    }

    fn number(&mut self) -> Result<(), CompileError> {
        let start = self.at;
        let digits = |parser: &mut Self| {
            while parser.bytes.get(parser.at).is_some_and(u8::is_ascii_digit) {
                parser.at += 1;
            }
        };
        digits(self);
        if self.bytes.get(self.at) == Some(&b'.') {
            self.at += 1;
            digits(self);
        }
        if matches!(self.bytes.get(self.at), Some(b'e' | b'E')) {
            let mark = self.at;
            self.at += 1;
            if matches!(self.bytes.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
                digits(self);
            } else {
                // Not an exponent after all, so let the caller report what follows.
                self.at = mark;
            }
        }
        let value: f64 = self.source[start..self.at]
            .parse()
            .map_err(|_| self.error("malformed number"))?;
        if !value.is_finite() {
            return Err(self.error("the number is too large"));
        }
        self.emit(Op::Const(value), 0, 1)
    }

    fn string_argument(&mut self) -> Result<String, CompileError> {
        self.skip_space();
        if self.bytes.get(self.at) != Some(&b'"') {
            return Err(self.error("expected a quoted name"));
        }
        self.at += 1;
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(|c| *c != b'"') {
            self.at += 1;
        }
        if self.at == self.bytes.len() {
            return Err(self.error("unterminated string"));
        }
        let text = self.source[start..self.at].to_owned();
        self.at += 1;
        Ok(text)
    }

    fn integer_argument(&mut self) -> Result<u64, CompileError> {
        self.skip_space();
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        self.source[start..self.at]
            .parse()
            .map_err(|_| self.error("expected a whole number"))
    }

    fn close_call(&mut self) -> Result<(), CompileError> {
        if self.eat(")") {
            Ok(())
        } else {
            Err(self.error("expected `)`"))
        }
    }

    fn expect_comma(&mut self) -> Result<(), CompileError> {
        if self.eat(",") {
            Ok(())
        } else {
            Err(self.error("expected `,`"))
        }
    }

    fn call(&mut self) -> Result<(), CompileError> {
        let name = self.word().to_owned();
        if !self.eat("(") {
            return Err(self.error("expected `(` after a function name"));
        }
        match name.as_str() {
            "ch" => self.channel_call(),
            "npoints" | "nprims" => {
                let input = self.input_argument()?;
                self.close_call()?;
                let stat = if name == "npoints" {
                    InputStat::Points
                } else {
                    InputStat::Prims
                };
                self.emit_input_stat(input, stat)
            }
            "bbox" => self.bbox_call(),
            _ => {
                let (func, arity) =
                    Func::lookup(&name).ok_or_else(|| self.error("unknown function"))?;
                for index in 0..arity {
                    if index > 0 {
                        self.expect_comma()?;
                    }
                    self.nested(Self::or_expression)?;
                }
                self.close_call()?;
                self.emit(Op::Call(func), arity, 1)
            }
        }
    }

    fn input_argument(&mut self) -> Result<u32, CompileError> {
        u32::try_from(self.integer_argument()?)
            .map_err(|_| self.error("the input index is too large"))
    }

    fn emit_input_stat(&mut self, input: u32, stat: InputStat) -> Result<(), CompileError> {
        self.out.uses_input_stats = true;
        self.emit(Op::InputStat(input, stat), 0, 1)
    }

    /// `ch("name")` reads this node; `ch(12, "name")` reads node 12. A vector component is named
    /// like `"size.x"`.
    fn channel_call(&mut self) -> Result<(), CompileError> {
        self.skip_space();
        let node = if self.bytes.get(self.at) == Some(&b'"') {
            None
        } else {
            let id = self.integer_argument()?;
            self.expect_comma()?;
            Some(id)
        };
        let name = self.string_argument()?;
        self.close_call()?;
        let index = u32::try_from(self.out.channels.len())
            .map_err(|_| self.error("too many channel references"))?;
        self.out.channels.push((node, name));
        self.emit(Op::Channel(index), 0, 1)
    }

    /// `bbox(0, "xmin")`, or `xmax`, `xsize`, `xcenter`, and the same for `y` and `z`.
    fn bbox_call(&mut self) -> Result<(), CompileError> {
        let input = self.input_argument()?;
        self.expect_comma()?;
        let selector = self.string_argument()?;
        self.close_call()?;
        let invalid = || self.error("expected a selector like \"xmin\", \"ysize\", or \"zcenter\"");
        let (axis, part) = selector.split_at_checked(1).ok_or_else(invalid)?;
        let axis = match axis {
            "x" => 0,
            "y" => 1,
            "z" => 2,
            _ => return Err(invalid()),
        };
        let stat = match part {
            "min" => InputStat::BoundsMin(axis),
            "max" => InputStat::BoundsMax(axis),
            "size" => InputStat::BoundsSize(axis),
            "center" => InputStat::BoundsCenter(axis),
            _ => return Err(invalid()),
        };
        self.emit_input_stat(input, stat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    struct TestScope;

    impl Scope for TestScope {
        fn frame(&self) -> f64 {
            48.0
        }
        fn fps(&self) -> f64 {
            24.0
        }
        fn channel(&self, node: Option<u64>, name: &str) -> Option<f64> {
            match (node, name) {
                (None, "own") => Some(3.0),
                (Some(7), "radius") => Some(2.5),
                (Some(7), "size.y") => Some(4.0),
                _ => None,
            }
        }
        fn input_stat(&self, input: u32, stat: InputStat) -> Option<f64> {
            match (input, stat) {
                (0, InputStat::Points) => Some(100.0),
                (0, InputStat::Prims) => Some(12.0),
                (0, InputStat::BoundsMin(1)) => Some(-1.0),
                (0, InputStat::BoundsMax(2)) => Some(5.0),
                (0, InputStat::BoundsSize(0)) => Some(8.0),
                (0, InputStat::BoundsCenter(1)) => Some(0.5),
                _ => None,
            }
        }
    }

    fn eval(source: &str) -> Result<f64, String> {
        Expr::compile(source)
            .map_err(|e| format!("compile: {e}"))?
            .eval(&TestScope)
            .map_err(|e| format!("eval: {e}"))
    }

    fn assert_value(source: &str, expected: f64) {
        match eval(source) {
            Ok(value) => assert!(
                (value - expected).abs() <= 1e-12 * expected.abs().max(1.0),
                "{source} = {value}, expected {expected}"
            ),
            Err(error) => assert_eq!(error, "", "{source}"),
        }
    }

    #[test]
    fn arithmetic_follows_conventional_precedence_and_associativity() {
        for (source, expected) in [
            ("1 + 2 * 3", 7.0),
            ("(1 + 2) * 3", 9.0),
            ("10 - 3 - 2", 5.0),
            ("24 / 4 / 2", 3.0),
            ("7 % 4", 3.0),
            ("2 ^ 3", 8.0),
            ("2 ^ 3 ^ 2", 512.0),
            ("-2 ^ 2", -4.0),
            ("2 ^ -1", 0.5),
            ("--3", 3.0),
            ("+3", 3.0),
            ("2 * -3", -6.0),
            ("1.5e2", 150.0),
            (".5", 0.5),
            ("1e-2", 0.01),
            ("  1  +\t2\n", 3.0),
        ] {
            assert_value(source, expected);
        }
    }

    #[test]
    fn comparisons_and_logic_produce_one_or_zero() {
        for (source, expected) in [
            ("1 < 2", 1.0),
            ("2 < 1", 0.0),
            ("2 <= 2", 1.0),
            ("3 <= 2", 0.0),
            ("3 > 2", 1.0),
            ("2 > 3", 0.0),
            ("2 >= 2", 1.0),
            ("1 >= 2", 0.0),
            ("2 == 2", 1.0),
            ("2 == 3", 0.0),
            ("2 != 3", 1.0),
            ("2 != 2", 0.0),
            ("1 && 2", 1.0),
            ("1 && 0", 0.0),
            ("0 || 2", 1.0),
            ("0 || 0", 0.0),
            ("!0", 1.0),
            ("!5", 0.0),
            ("1 + 1 == 2 && 2 * 2 > 3", 1.0),
            ("1 || 0 && 0", 1.0),
            ("!1 != 1", 1.0),
        ] {
            assert_value(source, expected);
        }
    }

    #[test]
    fn functions_compute_their_documented_results() {
        for (source, expected) in [
            ("sin($PI / 2)", 1.0),
            ("cos(0)", 1.0),
            ("tan(0)", 0.0),
            ("asin(1)", PI / 2.0),
            ("acos(1)", 0.0),
            ("atan(1)", PI / 4.0),
            ("atan2(1, 1)", PI / 4.0),
            ("sqrt(9)", 3.0),
            ("abs(-4)", 4.0),
            ("floor(1.7)", 1.0),
            ("floor(-1.2)", -2.0),
            ("ceil(1.2)", 2.0),
            ("round(1.5)", 2.0),
            ("frac(1.25)", 0.25),
            ("frac(-0.25)", 0.75),
            ("exp(0)", 1.0),
            ("log($E)", 1.0),
            ("log10(1000)", 3.0),
            ("deg($PI)", 180.0),
            ("rad(180)", PI),
            ("sign(-2)", -1.0),
            ("sign(2)", 1.0),
            ("sign(0)", 0.0),
            ("min(1, 2)", 1.0),
            ("max(1, 2)", 2.0),
            ("pow(2, 10)", 1024.0),
            ("fmod(7, 4)", 3.0),
            ("step(1, 2)", 1.0),
            ("step(2, 1)", 0.0),
            ("clamp(5, 0, 3)", 3.0),
            ("clamp(-5, 0, 3)", 0.0),
            ("clamp(2, 0, 3)", 2.0),
            ("lerp(10, 20, 0.25)", 12.5),
            ("smoothstep(0, 1, 0.5)", 0.5),
            ("smoothstep(0, 1, 2)", 1.0),
            ("smoothstep(0, 1, -2)", 0.0),
            ("if(1, 10, 20)", 10.0),
            ("if(0, 10, 20)", 20.0),
            ("fit(5, 0, 10, 100, 200)", 150.0),
            ("fit(50, 0, 10, 100, 200)", 200.0),
            ("fit(-50, 0, 10, 100, 200)", 100.0),
        ] {
            assert_value(source, expected);
        }
    }

    #[test]
    fn rand_is_uniform_in_range_and_repeatable() -> Result<(), String> {
        assert_eq!(eval("rand(3)")?, eval("rand(3)")?);
        assert_ne!(eval("rand(3)")?, eval("rand(4)")?);
        let mut sum = 0.0;
        for i in 0..2000 {
            let value = eval(&format!("rand({i})"))?;
            assert!((0.0..1.0).contains(&value), "{value}");
            sum += value;
        }
        assert!((sum / 2000.0 - 0.5).abs() < 0.04, "mean {}", sum / 2000.0);
        Ok(())
    }

    #[test]
    fn variables_and_scope_queries_read_the_scope() {
        for (source, expected) in [
            ("$F", 48.0),
            ("$T", 2.0),
            ("$FPS", 24.0),
            ("$F * 0.5 + 1", 25.0),
            ("ch(\"own\")", 3.0),
            ("ch(7, \"radius\") * 2", 5.0),
            ("ch(7, \"size.y\")", 4.0),
            ("npoints(0)", 100.0),
            ("nprims(0)", 12.0),
            ("bbox(0, \"ymin\")", -1.0),
            ("bbox(0, \"zmax\")", 5.0),
            ("bbox(0, \"xsize\")", 8.0),
            ("bbox(0, \"ycenter\")", 0.5),
        ] {
            assert_value(source, expected);
        }
    }

    #[test]
    fn runtime_faults_are_reported_rather_than_propagated() {
        let fails = |source: &str, expected: EvalError| match Expr::compile(source) {
            Ok(expr) => assert_eq!(expr.eval(&TestScope), Err(expected), "{source}"),
            Err(error) => assert_eq!(error.message, "", "{source}"),
        };
        fails("1 / 0", EvalError::NonFinite);
        fails("sqrt(-1)", EvalError::NonFinite);
        fails("log(0)", EvalError::NonFinite);
        fails("0 / 0", EvalError::NonFinite);
        fails("1e308 * 1e308", EvalError::NonFinite);
        fails(
            "ch(\"missing\")",
            EvalError::UnknownChannel("missing".to_owned()),
        );
        fails(
            "ch(9, \"radius\")",
            EvalError::UnknownChannel("radius".to_owned()),
        );
        fails("npoints(3)", EvalError::NoSuchInput(3));
        // A discarded branch may be non-finite without failing the whole.
        assert_value("if(1, 2, 1 / 0)", 2.0);
        // Negative zero settles, so that results hash consistently.
        assert_eq!(eval("-0").map(f64::to_bits), Ok(0f64.to_bits()));
    }

    #[test]
    fn analysis_reports_dependencies_and_time_use() -> Result<(), CompileError> {
        let refs = |source: &str| -> Result<Vec<u64>, CompileError> {
            Ok(Expr::compile(source)?.referenced_nodes().collect())
        };
        assert_eq!(refs("ch(3, \"a\") + ch(9, \"b\") + ch(\"c\")")?, vec![3, 9]);
        assert_eq!(refs("1 + 2")?, Vec::<u64>::new());
        for (source, expected) in [
            ("$F", true),
            ("$T * 2", true),
            ("$FPS", true),
            ("$PI", false),
            ("ch(1, \"a\")", false),
            ("sin(2)", false),
        ] {
            assert_eq!(
                Expr::compile(source)?.is_time_dependent(),
                expected,
                "{source}"
            );
        }
        assert!(Expr::compile("npoints(0)")?.reads_inputs());
        assert!(Expr::compile("bbox(0, \"xmin\")")?.reads_inputs());
        assert!(!Expr::compile("$F")?.reads_inputs());
        Ok(())
    }

    #[test]
    fn malformed_source_fails_at_compile_time_with_a_position() {
        for source in [
            "",
            "   ",
            "1 +",
            "* 2",
            "(1",
            "1)",
            "1 2",
            "$",
            "$X",
            "$f",
            "nosuch(1)",
            "sin",
            "sin()",
            "sin(1, 2)",
            "min(1)",
            "if(1, 2)",
            "fit(1, 2, 3, 4)",
            "ch()",
            "ch(1)",
            "ch(\"unterminated",
            "ch(1.5, \"a\")",
            "npoints()",
            "npoints(\"a\")",
            "npoints(99999999999)",
            "bbox(0)",
            "bbox(0, \"wmin\")",
            "bbox(0, \"xmiddle\")",
            "bbox(0, \"\")",
            "1e999",
            "1 ? 2 : 3",
            "a",
            "1 = 2",
            "1..2",
            "é",
        ] {
            assert!(Expr::compile(source).is_err(), "{source:?} compiled");
        }
        assert_eq!(Expr::compile("1 + $X").map_err(|e| e.position), Err(6));
        assert_eq!(Expr::compile("(1 2").map_err(|e| e.position), Err(3));
        // A stray symbol is not a function with a missing name.
        assert_eq!(
            Expr::compile("1 + #").map_err(|e| e.message),
            Err("expected a value".to_owned())
        );
    }

    #[test]
    fn hostile_input_is_rejected_instead_of_exhausting_resources() {
        // Valid in every way except its length.
        let too_long = format!("1{}", "+1".repeat(MAX_SOURCE_LEN));
        assert_eq!(
            Expr::compile(&too_long).map_err(|e| e.message),
            Err("the expression is too long".to_owned())
        );
        // Function arguments pile up on the evaluation stack with little parser nesting, so only
        // the stack limit stands between this and an out-of-bounds write at evaluation.
        let wide = format!("{}0{}", "fit(1, 2, 3, 4, ".repeat(30), ")".repeat(30));
        assert_eq!(
            Expr::compile(&wide).map_err(|e| e.message),
            Err("the expression is too deeply nested".to_owned())
        );
        // Just inside the stack limit, it evaluates without trouble.
        let fits = format!("{}0{}", "fit(1, 2, 3, 4, ".repeat(15), ")".repeat(15));
        assert!(Expr::compile(&fits).is_ok_and(|expr| expr.eval(&TestScope).is_ok()));
        // Each form attacks a different recursion path or the evaluation stack.
        for source in [
            format!("{}1{}", "(".repeat(2000), ")".repeat(2000)),
            format!("{}1", "-".repeat(2000)),
            format!("{}1", "!".repeat(2000)),
            format!("{}1", "+".repeat(2000)),
            format!("{}1{}", "abs(".repeat(1000), ")".repeat(1000)),
            format!("{}1", "1^".repeat(1000)),
            // Right-leaning without parentheses, which needs depth on the evaluation stack.
            format!("{}1{}", "1+(".repeat(70), ")".repeat(70)),
        ] {
            assert!(Expr::compile(&source).is_err(), "{}", &source[..12]);
        }
        // Deep but legal nesting works right up to the limit.
        assert_value(&format!("{}7{}", "(".repeat(40), ")".repeat(40)), 7.0);
        // A long flat expression needs little stack, and evaluates fine.
        assert_value(&format!("1{}", "+1".repeat(1000)), 1001.0);
    }

    #[test]
    fn arbitrary_bytes_never_panic_at_compile_or_evaluation() {
        let rng = Philox4x32::new(0xE1);
        let alphabet: Vec<char> = "0123456789.eE+-*/%^()<>=!&|,\"$FTPI chsinbox_ \té"
            .chars()
            .collect();
        for round in 0..20_000u32 {
            let r = rng.generate([round, 0, 0, 0]);
            let length = (r[0] % 24) as usize;
            let source: String = (0..length)
                .map(|i| {
                    let pick = rng.generate([round, i as u32 + 1, 0, 0])[0] as usize;
                    alphabet[pick % alphabet.len()]
                })
                .collect();
            if let Ok(expr) = Expr::compile(&source) {
                // Any result is acceptable as long as it returns: a number must be finite.
                if let Ok(value) = expr.eval(&TestScope) {
                    assert!(value.is_finite(), "{source}");
                }
            }
        }
    }

    #[test]
    fn range_functions_work_away_from_zero() {
        // A range starting at zero hides mistakes in the subtraction, so these do not.
        for (source, expected) in [
            ("smoothstep(2, 4, 3)", 0.5),
            ("smoothstep(2, 6, 3)", 0.15625),
            ("smoothstep(2, 4, 1)", 0.0),
            ("smoothstep(2, 4, 5)", 1.0),
            ("fit(3, 2, 6, 10, 20)", 12.5),
            ("fit(1, 2, 6, 10, 20)", 10.0),
            ("fit(7, 2, 6, 10, 20)", 20.0),
        ] {
            assert_value(source, expected);
        }
    }

    #[test]
    fn every_value_kind_works_on_either_side_of_an_operator() {
        // On the right, a value lands above another on the stack, where a wrong stack pointer shows.
        for (source, expected) in [
            ("2 + $F", 50.0),
            ("2 + $T", 4.0),
            ("2 + $FPS", 26.0),
            ("2 + $PI - $PI", 2.0),
            ("2 + ch(\"own\")", 5.0),
            ("2 + npoints(0)", 102.0),
            ("2 + bbox(0, \"xsize\")", 10.0),
            ("$F + 2", 50.0),
            ("$T + 2", 4.0),
            ("$FPS + 2", 26.0),
            ("$F + $T + $FPS", 74.0),
            ("min($F, $T)", 2.0),
        ] {
            assert_value(source, expected);
        }
    }

    #[test]
    fn strict_comparisons_are_false_at_equality() {
        assert_value("2 < 2", 0.0);
        assert_value("2 > 2", 0.0);
        assert_value("2 <= 2", 1.0);
        assert_value("2 >= 2", 1.0);
        assert_value("step(2, 2)", 1.0);
    }

    #[test]
    fn limits_hold_at_their_exact_boundaries() {
        // Source length: exactly at the limit is fine, one more is not.
        let padded = |len: usize| format!("1{}", " ".repeat(len - 1));
        assert!(Expr::compile(&padded(MAX_SOURCE_LEN)).is_ok());
        assert_eq!(
            Expr::compile(&padded(MAX_SOURCE_LEN + 1)).map_err(|e| e.message),
            Err("the expression is too long".to_owned())
        );
        // Parser nesting: each parenthesis adds one level, up to the limit.
        let nested = |levels: usize| format!("{}1{}", "(".repeat(levels), ")".repeat(levels));
        assert!(Expr::compile(&nested(MAX_STACK)).is_ok());
        assert_eq!(
            Expr::compile(&nested(MAX_STACK + 1)).map_err(|e| e.message),
            Err("the expression is too deeply nested".to_owned())
        );
        // Stack depth: each `fit` holds four values while its last argument evaluates, so 15 levels
        // hold 60. An innermost formula needing four more reaches the limit of 64 exactly.
        let levels =
            |inner: &str| format!("{}{inner}{}", "fit(1, 2, 3, 4, ".repeat(15), ")".repeat(15));
        let at_limit = Expr::compile(&levels("min(1, min(2, min(3, 4)))"));
        // Filling the very last slot still evaluates in bounds.
        assert!(at_limit.is_ok_and(|expr| expr.eval(&TestScope).is_ok()));
        assert_eq!(
            Expr::compile(&levels("min(1, min(2, min(3, min(4, 5))))")).map_err(|e| e.message),
            Err("the expression is too deeply nested".to_owned())
        );
    }

    #[test]
    fn faults_describe_themselves_because_agents_read_the_messages() {
        let compile = Expr::compile("1 + $X")
            .map(|_| ())
            .map_err(|e| e.to_string());
        assert_eq!(
            compile,
            Err("at byte 6: unknown variable: use $F, $T, $FPS, $PI, or $E".to_owned())
        );
        assert_eq!(
            EvalError::UnknownChannel("size.x".to_owned()).to_string(),
            "no such channel: size.x"
        );
        assert_eq!(EvalError::NoSuchInput(2).to_string(), "no input 2");
        assert_eq!(EvalError::NonFinite.to_string(), "the result is not finite");
    }
}
