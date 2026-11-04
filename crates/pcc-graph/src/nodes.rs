//! Nodes as data, described by a table of definitions.
//!
//! A [`Node`] is an operator name, input connections, and named parameters. A [`NodeDef`] declares
//! an operator's ports, parameter kinds, and constraints, and the catalog, validation, `.ofxs`
//! serialization, and cache keys all derive from it. Adding an operator takes one table row and one
//! cook function, and [`register_node_def`] lets that happen at runtime, as plugins will need.

use crate::SceneError;
use pcc_expr::Expr;
use pcc_id::NodeId;
use pcc_math::Vec3;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{OnceLock, RwLock},
};

/// One parameter's value. Untagged, so JSON stays plain: `"rows": 16`, `"radius": 1.5`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Uint(u64),
    Float(f32),
    Vec3(Vec3),
    /// A vector with at least one expression component. Literal components are held as text too.
    VecExpr {
        x: String,
        y: String,
        z: String,
    },
    /// A formula evaluated per cook, such as `$F * 0.1`.
    Expr {
        expr: String,
    },
    Text(String),
}

impl ParamValue {
    /// Feeds a canonical byte form to `update`, for content-addressed cache keys.
    pub fn update_hash(&self, mut update: impl FnMut(&[u8])) {
        let float = |value: f32| {
            if value == 0.0 { 0.0f32 } else { value }
                .to_bits()
                .to_le_bytes()
        };
        match self {
            Self::Bool(value) => {
                update(&[1, u8::from(*value)]);
            }
            Self::Uint(value) => {
                update(&[2]);
                update(&value.to_le_bytes());
            }
            Self::Float(value) => {
                update(&[3]);
                update(&float(*value));
            }
            Self::Vec3(value) => {
                update(&[4]);
                update(&float(value.x));
                update(&float(value.y));
                update(&float(value.z));
            }
            Self::Text(value) => {
                update(&[5]);
                update(&(value.len() as u64).to_le_bytes());
                update(value.as_bytes());
            }
            Self::Expr { expr } => {
                update(&[6]);
                update(&(expr.len() as u64).to_le_bytes());
                update(expr.as_bytes());
            }
            Self::VecExpr { x, y, z } => {
                update(&[7]);
                for component in [x, y, z] {
                    update(&(component.len() as u64).to_le_bytes());
                    update(component.as_bytes());
                }
            }
        }
    }
}

/// How a parameter is typed and written in `.ofxs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    F32,
    U32,
    U64,
    Bool,
    Vec3,
    Text,
    /// One of a fixed set of lowercase tokens.
    Choice(&'static [&'static str]),
}

impl ParamKind {
    fn type_name(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::Bool => "bool",
            Self::Vec3 => "vec3",
            Self::Text | Self::Choice(_) => "string",
        }
    }
}

/// A constraint on one parameter. Floats and vectors must always be finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Check {
    Any,
    Positive,
    FloatAtLeast(f32),
    /// Inclusive at both ends.
    FloatRange(f32, f32),
    /// Exclusive below, inclusive above.
    FloatOpenClosed(f32, f32),
    /// Inclusive below, exclusive above.
    FloatClosedOpen(f32, f32),
    UintAtLeast(u32),
    /// Inclusive at both ends.
    UintRange(u32, u32),
    VecPositive,
    VecNoZeroComponent,
    VecNonZero,
    Identifier,
    /// Non-blank ASCII source text; hex-encoded in `.ofxs`.
    Snippet,
    /// A BLAKE3 hash in canonical lowercase hexadecimal.
    ContentHash,
}

#[derive(Clone, Copy, Debug)]
pub struct NodeInput {
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct NodeParam {
    pub name: &'static str,
    pub kind: ParamKind,
    pub check: Check,
    pub description: &'static str,
}

const fn input(name: &'static str, description: &'static str) -> NodeInput {
    NodeInput { name, description }
}

const fn param(
    name: &'static str,
    kind: ParamKind,
    check: Check,
    description: &'static str,
) -> NodeParam {
    NodeParam {
        name,
        kind,
        check,
        description,
    }
}

/// A constraint spanning several of a node's parameters.
pub type NodeRule = fn(&Node) -> Result<(), SceneError>;

/// An operator's complete description.
#[derive(Clone, Copy, Debug)]
pub struct NodeDef {
    pub type_name: &'static str,
    pub category: &'static str,
    pub summary: &'static str,
    pub inputs: &'static [NodeInput],
    pub params: &'static [NodeParam],
    /// The operator mixes its node id into its result, so the id must key its cache entries.
    pub seeded_by_node_id: bool,
    /// The operator reads the current time, so its results cannot be reused across frames.
    pub time_dependent: bool,
    /// A constraint spanning several parameters.
    pub rule: Option<NodeRule>,
}

impl NodeDef {
    #[must_use]
    pub const fn new(
        type_name: &'static str,
        category: &'static str,
        summary: &'static str,
        inputs: &'static [NodeInput],
        params: &'static [NodeParam],
    ) -> Self {
        Self {
            type_name,
            category,
            summary,
            inputs,
            params,
            seeded_by_node_id: false,
            time_dependent: false,
            rule: None,
        }
    }

    #[must_use]
    pub const fn seeded_by_node_id(mut self) -> Self {
        self.seeded_by_node_id = true;
        self
    }

    #[must_use]
    pub const fn time_dependent(mut self) -> Self {
        self.time_dependent = true;
        self
    }

    #[must_use]
    pub const fn with_rule(mut self, rule: NodeRule) -> Self {
        self.rule = Some(rule);
        self
    }

    /// Checks a node's shape and every parameter against this definition.
    pub fn validate(&self, node: &Node) -> Result<(), SceneError> {
        if node.inputs.len() != self.inputs.len() || node.params.len() != self.params.len() {
            return Err(SceneError::Field);
        }
        for param in self.params {
            let value = node.params.get(param.name).ok_or(SceneError::Field)?;
            validate_param(param, value)?;
        }
        // A rule compares values, which an unresolved expression does not have yet. The evaluator
        // applies this whole check again to the resolved node.
        match self.rule {
            Some(rule) if !node.has_expressions() => rule(node),
            _ => Ok(()),
        }
    }

    fn spec(&self) -> crate::NodeTypeSpec {
        crate::NodeTypeSpec {
            type_name: self.type_name.to_owned(),
            category: self.category.to_owned(),
            summary: self.summary.to_owned(),
            inputs: self
                .inputs
                .iter()
                .map(|port| crate::NodePortSpec {
                    name: port.name.to_owned(),
                    description: port.description.to_owned(),
                })
                .collect(),
            outputs: vec![crate::NodePortSpec {
                name: "out".to_owned(),
                description: "Output geometry stream.".to_owned(),
            }],
            params: self
                .params
                .iter()
                .map(|param| crate::NodeParamSpec {
                    name: param.name.to_owned(),
                    param_type: param.kind.type_name().to_owned(),
                    description: param.description.to_owned(),
                })
                .collect(),
        }
    }
}

fn check_finite(value: f32) -> Result<f32, SceneError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(SceneError::NonFinite)
    }
}

fn require(condition: bool) -> Result<(), SceneError> {
    if condition {
        Ok(())
    } else {
        Err(SceneError::Field)
    }
}

fn validate_expression(source: &str) -> Result<(), SceneError> {
    Expr::compile(source)
        .map(|_| ())
        .map_err(|_| SceneError::Field)
}

fn validate_param(param: &NodeParam, value: &ParamValue) -> Result<(), SceneError> {
    match value {
        // Only the syntax can be judged now. Ranges wait until the value exists.
        ParamValue::Expr { expr } => {
            require(matches!(
                param.kind,
                ParamKind::F32 | ParamKind::U32 | ParamKind::U64 | ParamKind::Bool
            ))?;
            return validate_expression(expr);
        }
        ParamValue::VecExpr { x, y, z } => {
            require(param.kind == ParamKind::Vec3)?;
            validate_expression(x)?;
            validate_expression(y)?;
            return validate_expression(z);
        }
        _ => {}
    }
    match param.kind {
        ParamKind::F32 => {
            let number = check_finite(as_f32(value).ok_or(SceneError::Field)?)?;
            match param.check {
                Check::Positive => require(number > 0.0),
                Check::FloatAtLeast(low) => require(number >= low),
                Check::FloatRange(low, high) => require(number >= low && number <= high),
                Check::FloatOpenClosed(low, high) => require(number > low && number <= high),
                Check::FloatClosedOpen(low, high) => require(number >= low && number < high),
                _ => Ok(()),
            }
        }
        ParamKind::U32 => {
            let number = as_u32(value).ok_or(SceneError::Field)?;
            match param.check {
                Check::UintAtLeast(low) => require(number >= low),
                Check::UintRange(low, high) => require(number >= low && number <= high),
                _ => Ok(()),
            }
        }
        ParamKind::U64 => as_u64(value).map(|_| ()).ok_or(SceneError::Field),
        ParamKind::Bool => require(matches!(value, ParamValue::Bool(_))),
        ParamKind::Vec3 => {
            let ParamValue::Vec3(vector) = value else {
                return Err(SceneError::Field);
            };
            Vec3::checked(vector.x, vector.y, vector.z).map_err(|_| SceneError::NonFinite)?;
            let components = [vector.x, vector.y, vector.z];
            match param.check {
                Check::VecPositive => require(components.iter().all(|&c| c > 0.0)),
                Check::VecNoZeroComponent => require(components.iter().all(|&c| c != 0.0)),
                Check::VecNonZero => require(components.iter().any(|&c| c != 0.0)),
                _ => Ok(()),
            }
        }
        ParamKind::Text => {
            let ParamValue::Text(text) = value else {
                return Err(SceneError::Field);
            };
            match param.check {
                Check::Identifier => validate_identifier(text),
                Check::Snippet => require(!text.trim().is_empty() && text.is_ascii()),
                Check::ContentHash => require(
                    text.len() == 64
                        && text
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                ),
                _ => Ok(()),
            }
        }
        ParamKind::Choice(tokens) => {
            let ParamValue::Text(text) = value else {
                return Err(SceneError::Field);
            };
            require(tokens.contains(&text.as_str()))
        }
    }
}

fn validate_identifier(name: &str) -> Result<(), SceneError> {
    let mut chars = name.chars();
    let valid_start = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    require(name != "P" && valid_start && chars.all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

const fn as_f32(value: &ParamValue) -> Option<f32> {
    match value {
        ParamValue::Float(number) => Some(*number),
        ParamValue::Uint(number) => Some(*number as f32),
        _ => None,
    }
}

const fn as_u64(value: &ParamValue) -> Option<u64> {
    match value {
        ParamValue::Uint(number) => Some(*number),
        _ => None,
    }
}

fn as_u32(value: &ParamValue) -> Option<u32> {
    as_u64(value).and_then(|number| u32::try_from(number).ok())
}

/// A node in the graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Node {
    pub id: NodeId,
    pub op: String,
    #[serde(default)]
    pub inputs: Vec<NodeId>,
    #[serde(default)]
    pub params: BTreeMap<String, ParamValue>,
}

impl Node {
    #[must_use]
    pub fn new(op: &str, id: NodeId) -> Self {
        Self {
            id,
            op: op.to_owned(),
            inputs: Vec::new(),
            params: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_input(mut self, input: NodeId) -> Self {
        self.inputs.push(input);
        self
    }

    fn with_value(mut self, name: &str, value: ParamValue) -> Self {
        self.params.insert(name.to_owned(), value);
        self
    }

    #[must_use]
    pub fn with_f32(self, name: &str, value: f32) -> Self {
        self.with_value(name, ParamValue::Float(value))
    }

    #[must_use]
    pub fn with_u32(self, name: &str, value: u32) -> Self {
        self.with_value(name, ParamValue::Uint(u64::from(value)))
    }

    #[must_use]
    pub fn with_u64(self, name: &str, value: u64) -> Self {
        self.with_value(name, ParamValue::Uint(value))
    }

    #[must_use]
    pub fn with_bool(self, name: &str, value: bool) -> Self {
        self.with_value(name, ParamValue::Bool(value))
    }

    #[must_use]
    pub fn with_vec3(self, name: &str, value: Vec3) -> Self {
        self.with_value(name, ParamValue::Vec3(value))
    }

    /// Sets a scalar parameter to a formula such as `$F * 0.1`.
    #[must_use]
    pub fn with_expr(self, name: &str, expr: impl Into<String>) -> Self {
        self.with_value(name, ParamValue::Expr { expr: expr.into() })
    }

    /// Sets a vector parameter per component; each is a formula or a plain number.
    #[must_use]
    pub fn with_vec3_expr(self, name: &str, [x, y, z]: [&str; 3]) -> Self {
        self.with_value(
            name,
            ParamValue::VecExpr {
                x: canonical_component(x),
                y: canonical_component(y),
                z: canonical_component(z),
            },
        )
    }

    #[must_use]
    pub fn with_text(self, name: &str, value: impl Into<String>) -> Self {
        self.with_value(name, ParamValue::Text(value.into()))
    }

    #[must_use]
    pub const fn id(&self) -> NodeId {
        self.id
    }

    /// A generator is an operator with no input ports.
    #[must_use]
    pub fn is_generator(&self) -> bool {
        self.inputs.is_empty()
    }

    #[must_use]
    pub fn inputs(&self) -> Vec<NodeId> {
        self.inputs.clone()
    }

    /// Whether any parameter needs evaluating before this node can cook.
    #[must_use]
    pub fn has_expressions(&self) -> bool {
        self.params
            .values()
            .any(|value| matches!(value, ParamValue::Expr { .. } | ParamValue::VecExpr { .. }))
    }

    /// Everything that must evaluate first: input geometry, and nodes that `ch` reads.
    #[must_use]
    pub fn dependencies(&self) -> Vec<NodeId> {
        let mut found = self.inputs.clone();
        let mut add = |source: &str| {
            if let Ok(expr) = Expr::compile(source) {
                found.extend(expr.referenced_nodes().map(NodeId));
            }
        };
        for value in self.params.values() {
            match value {
                ParamValue::Expr { expr } => add(expr),
                ParamValue::VecExpr { x, y, z } => {
                    add(x);
                    add(y);
                    add(z);
                }
                _ => {}
            }
        }
        found
    }

    #[must_use]
    pub fn f32(&self, name: &str) -> Option<f32> {
        as_f32(self.params.get(name)?)
    }

    #[must_use]
    pub fn u32(&self, name: &str) -> Option<u32> {
        as_u32(self.params.get(name)?)
    }

    #[must_use]
    pub fn u64(&self, name: &str) -> Option<u64> {
        as_u64(self.params.get(name)?)
    }

    #[must_use]
    pub fn boolean(&self, name: &str) -> Option<bool> {
        match self.params.get(name)? {
            ParamValue::Bool(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn vec3(&self, name: &str) -> Option<Vec3> {
        match self.params.get(name)? {
            ParamValue::Vec3(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn text(&self, name: &str) -> Option<&str> {
        match self.params.get(name)? {
            ParamValue::Text(value) => Some(value),
            _ => None,
        }
    }

    /// JSON has one number type, so `"radius": 1` arrives as an integer. This stores it as the
    /// parameter's declared kind, so that equal scenes compare and hash equal however they arrived.
    pub fn canonicalize_numbers(&mut self) {
        let Some(def) = node_def(&self.op) else {
            return;
        };
        for param in def.params {
            let Some(value) = self.params.get_mut(param.name) else {
                continue;
            };
            if param.kind == ParamKind::F32
                && let Some(number) = as_f32(value)
            {
                *value = ParamValue::Float(number);
            }
            if let ParamValue::VecExpr { x, y, z } = value {
                *x = canonical_component(x);
                *y = canonical_component(y);
                *z = canonical_component(z);
                // A vector of plain numbers is just a vector, however it arrived.
                if let (Ok(x), Ok(y), Ok(z)) = (x.parse(), y.parse(), z.parse()) {
                    *value = ParamValue::Vec3(Vec3 { x, y, z });
                }
            }
        }
    }

    /// Appends this node's `.ofxs` line: operator, id, inputs, then parameters in table order.
    pub(crate) fn write_ofxs_line(&self, out: &mut String) {
        let canonical = |v: f32| if v == 0.0 { 0.0 } else { v };
        out.push_str(&format!("{} {}", self.op, self.id.0));
        for input in &self.inputs {
            out.push_str(&format!(" {}", input.0));
        }
        let Some(def) = node_def(&self.op) else {
            out.push('\n');
            return;
        };
        for param in def.params {
            let Some(value) = self.params.get(param.name) else {
                continue;
            };
            match value {
                ParamValue::Bool(v) => out.push_str(&format!(" {v}")),
                ParamValue::Uint(v) if param.kind == ParamKind::F32 => {
                    out.push_str(&format!(" {:e}", canonical(*v as f32)));
                }
                ParamValue::Uint(v) => out.push_str(&format!(" {v}")),
                ParamValue::Float(v) => out.push_str(&format!(" {:e}", canonical(*v))),
                ParamValue::Vec3(v) => out.push_str(&format!(
                    " {:e} {:e} {:e}",
                    canonical(v.x),
                    canonical(v.y),
                    canonical(v.z)
                )),
                ParamValue::Text(v) if param.check == Check::Snippet => {
                    out.push(' ');
                    out.push_str(&encode_hex(v.as_bytes()));
                }
                ParamValue::Text(v) => out.push_str(&format!(" {v}")),
                ParamValue::Expr { expr } => {
                    out.push_str(&format!(" ={}", encode_hex(expr.as_bytes())))
                }
                ParamValue::VecExpr { x, y, z } => {
                    for component in [x, y, z] {
                        if component.parse::<f32>().is_ok() {
                            out.push_str(&format!(" {component}"));
                        } else {
                            out.push_str(&format!(" ={}", encode_hex(component.as_bytes())));
                        }
                    }
                }
            }
        }
        out.push('\n');
    }

    /// Parses one `.ofxs` node line.
    pub(crate) fn parse_ofxs_line(line: &str) -> Result<Self, SceneError> {
        let mut parts = line.split(' ');
        let op = parse_next(&mut parts)?;
        let def = node_def(op).ok_or(SceneError::Field)?;
        let mut node = Self::new(op, NodeId(parse_u64_strict(parse_next(&mut parts)?)?));
        for _ in def.inputs {
            node.inputs
                .push(NodeId(parse_u64_strict(parse_next(&mut parts)?)?));
        }
        for param in def.params {
            let value = match param.kind {
                ParamKind::F32 | ParamKind::U32 | ParamKind::U64 | ParamKind::Bool => {
                    let token = parse_next(&mut parts)?;
                    match token.strip_prefix('=') {
                        Some(hex) => ParamValue::Expr {
                            expr: decode_hex_ascii(hex)?,
                        },
                        None => parse_scalar(param.kind, token)?,
                    }
                }
                ParamKind::Vec3 => {
                    let tokens = [
                        parse_next(&mut parts)?,
                        parse_next(&mut parts)?,
                        parse_next(&mut parts)?,
                    ];
                    if tokens.iter().any(|token| token.starts_with('=')) {
                        let [x, y, z] = tokens.map(parse_component);
                        ParamValue::VecExpr {
                            x: x?,
                            y: y?,
                            z: z?,
                        }
                    } else {
                        ParamValue::Vec3(Vec3 {
                            x: parse_f32(tokens[0])?,
                            y: parse_f32(tokens[1])?,
                            z: parse_f32(tokens[2])?,
                        })
                    }
                }
                ParamKind::Text if param.check == Check::Snippet => {
                    ParamValue::Text(decode_hex_ascii(parse_next(&mut parts)?)?)
                }
                ParamKind::Text | ParamKind::Choice(_) => {
                    ParamValue::Text(parse_next(&mut parts)?.to_owned())
                }
            };
            node.params.insert(param.name.to_owned(), value);
        }
        if parts.next().is_some() {
            return Err(SceneError::Field);
        }
        Ok(node)
    }
}

fn parse_scalar(kind: ParamKind, token: &str) -> Result<ParamValue, SceneError> {
    Ok(match kind {
        ParamKind::F32 => ParamValue::Float(parse_f32(token)?),
        ParamKind::U32 => ParamValue::Uint(u64::from(parse_u32_strict(token)?)),
        ParamKind::U64 => ParamValue::Uint(parse_u64_strict(token)?),
        _ => ParamValue::Bool(parse_bool(token)?),
    })
}

/// A vector component in a scene file: `=hex` for a formula, or a plain number.
fn parse_component(token: &str) -> Result<String, SceneError> {
    match token.strip_prefix('=') {
        Some(hex) => decode_hex_ascii(hex),
        None => Ok(canonical_component(&format!("{:e}", parse_f32(token)?))),
    }
}

/// Writes a plain-number component in canonical form, leaving formulas alone.
fn canonical_component(text: &str) -> String {
    match text.trim().parse::<f32>() {
        Ok(value) if value.is_finite() => {
            format!("{:e}", if value == 0.0 { 0.0 } else { value })
        }
        _ => text.to_owned(),
    }
}

fn parse_next<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Result<&'a str, SceneError> {
    match parts.next() {
        Some(part) if !part.is_empty() => Ok(part),
        _ => Err(SceneError::Field),
    }
}

pub(crate) fn parse_u64_strict(token: &str) -> Result<u64, SceneError> {
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_digit()) {
        return Err(SceneError::Field);
    }
    token.parse().map_err(|_| SceneError::Field)
}

fn parse_u32_strict(token: &str) -> Result<u32, SceneError> {
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_digit()) {
        return Err(SceneError::Field);
    }
    token.parse().map_err(|_| SceneError::Field)
}

fn parse_f32(token: &str) -> Result<f32, SceneError> {
    check_finite(token.parse().map_err(|_| SceneError::Field)?)
}

fn parse_bool(token: &str) -> Result<bool, SceneError> {
    match token {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(SceneError::Field),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

fn decode_hex_ascii(hex: &str) -> Result<String, SceneError> {
    let digits = hex.as_bytes();
    if digits.is_empty() || !digits.len().is_multiple_of(2) {
        return Err(SceneError::Field);
    }
    let nibble = |digit: u8| match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        _ => Err(SceneError::Field),
    };
    let mut bytes = Vec::with_capacity(digits.len() / 2);
    for pair in digits.chunks_exact(2) {
        bytes.push(nibble(pair[0])? * 16 + nibble(pair[1])?);
    }
    let text = String::from_utf8(bytes).map_err(|_| SceneError::Field)?;
    require(text.is_ascii())?;
    Ok(text)
}

fn domains_differ(node: &Node) -> Result<(), SceneError> {
    require(node.text("from_domain") != node.text("to_domain"))
}

fn major_exceeds_minor(node: &Node) -> Result<(), SceneError> {
    require(node.f32("major_radius") > node.f32("minor_radius"))
}

fn bounds_are_ordered(node: &Node) -> Result<(), SceneError> {
    require(node.text("domain") != Some("detail"))?;
    let (Some(low), Some(high)) = (node.vec3("min"), node.vec3("max")) else {
        return Err(SceneError::Field);
    };
    require(low.x <= high.x && low.y <= high.y && low.z <= high.z)
}

/// Why an operator could not be registered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeRegistrationError {
    AlreadyDefined,
    /// A type name becomes an `.ofxs` token, so it must be a lowercase identifier.
    InvalidName,
    /// A parameter name repeats, which would make a node's `.ofxs` line ambiguous.
    DuplicateParam,
}

fn registry() -> &'static RwLock<BTreeMap<&'static str, &'static NodeDef>> {
    static REGISTRY: OnceLock<RwLock<BTreeMap<&'static str, &'static NodeDef>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(BTreeMap::new()))
}

/// Looks up an operator, built-in or registered at runtime.
#[must_use]
pub fn node_def(op: &str) -> Option<&'static NodeDef> {
    if let Some(found) = BUILTIN_NODES.iter().find(|def| def.type_name == op) {
        return Some(found);
    }
    registry().read().ok()?.get(op).copied()
}

/// Adds an operator definition for the rest of the process. Built-ins cannot be replaced, so a
/// scene that uses only built-ins means the same thing whatever is loaded.
pub fn register_node_def(def: NodeDef) -> Result<(), NodeRegistrationError> {
    let name = def.type_name;
    let mut chars = name.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == ':');
    if !valid {
        return Err(NodeRegistrationError::InvalidName);
    }
    for (index, param) in def.params.iter().enumerate() {
        if def.params[..index].iter().any(|p| p.name == param.name) {
            return Err(NodeRegistrationError::DuplicateParam);
        }
    }
    if node_def(name).is_some() {
        return Err(NodeRegistrationError::AlreadyDefined);
    }
    let mut table = registry()
        .write()
        .map_err(|_| NodeRegistrationError::AlreadyDefined)?;
    table.insert(name, Box::leak(Box::new(def)));
    Ok(())
}

/// Every operator's description, built-ins first and in table order.
#[must_use]
pub fn node_catalog() -> Vec<crate::NodeTypeSpec> {
    let mut specs: Vec<_> = BUILTIN_NODES.iter().map(NodeDef::spec).collect();
    if let Ok(table) = registry().read() {
        specs.extend(table.values().map(|def| def.spec()));
    }
    specs
}

static BUILTIN_NODES: &[NodeDef] = &[
    NodeDef::new(
        "create",
        "generator",
        "Generates a deterministic sequence of points along the +X axis.",
        &[],
        &[param(
            "count",
            ParamKind::U32,
            Check::Any,
            "Number of points to create.",
        )],
    ),
    NodeDef::new(
        "grid",
        "generator",
        "Generates a right-handed Y-up quad polygon mesh in the XZ plane.",
        &[],
        &[
            param(
                "rows",
                ParamKind::U32,
                Check::UintAtLeast(2),
                "Vertex row count (>= 2).",
            ),
            param(
                "cols",
                ParamKind::U32,
                Check::UintAtLeast(2),
                "Vertex column count (>= 2).",
            ),
            param(
                "size_x",
                ParamKind::F32,
                Check::Positive,
                "Positive finite extent along X in metres.",
            ),
            param(
                "size_z",
                ParamKind::F32,
                Check::Positive,
                "Positive finite extent along Z in metres.",
            ),
        ],
    ),
    NodeDef::new(
        "box",
        "generator",
        "Generates a closed outward-oriented 8-point, 6-quad manifold box mesh.",
        &[],
        &[param(
            "size",
            ParamKind::Vec3,
            Check::VecPositive,
            "Positive finite (x, y, z) dimensions in metres.",
        )],
    ),
    NodeDef::new(
        "transform",
        "modifier",
        "Translates all input points by a finite 3D offset in metres.",
        &[input("input", "Upstream geometry stream to translate.")],
        &[param(
            "offset",
            ParamKind::Vec3,
            Check::Any,
            "Finite (x, y, z) translation offset in metres.",
        )],
    ),
    NodeDef::new(
        "normal",
        "modifier",
        "Computes area-weighted smooth point normals (`v@N`) across polygon faces.",
        &[input("input", "Upstream polygon mesh.")],
        &[],
    ),
    NodeDef::new(
        "wrangle",
        "modifier",
        "Executes a fuel-metered VEX-lite data-parallel snippet over all points.",
        &[input("input", "Upstream geometry stream.")],
        &[param(
            "snippet",
            ParamKind::Text,
            Check::Snippet,
            "VEX-lite point wrangle source code.",
        )],
    )
    .time_dependent(),
    NodeDef::new(
        "subdivide",
        "modifier",
        "Subdivides manifold polygon meshes using Catmull-Clark subdivision.",
        &[input("input", "Upstream manifold polygon mesh.")],
        &[param(
            "iterations",
            ParamKind::U32,
            Check::UintRange(1, 6),
            "Number of Catmull-Clark subdivision iterations (1..=6).",
        )],
    ),
    NodeDef::new(
        "extrude",
        "modifier",
        "Extrudes polygon faces along their face normals with optional inset and side-wall stitching.",
        &[input("input", "Upstream manifold polygon mesh.")],
        &[
            param(
                "distance",
                ParamKind::F32,
                Check::Any,
                "Finite extrusion distance along face normals in metres.",
            ),
            param(
                "inset",
                ParamKind::F32,
                Check::FloatAtLeast(0.0),
                "Non-negative finite inset toward face centroids in metres.",
            ),
        ],
    ),
    NodeDef::new(
        "attribpromote",
        "modifier",
        "Promotes F32 or Vec3 attributes between point, prim, and detail domains.",
        &[input("input", "Upstream geometry stream.")],
        &[
            param(
                "name",
                ParamKind::Text,
                Check::Identifier,
                "Attribute name to promote.",
            ),
            param(
                "from_domain",
                ParamKind::Choice(&["point", "prim", "detail"]),
                Check::Any,
                "Source domain (`point`, `prim`, `detail`).",
            ),
            param(
                "to_domain",
                ParamKind::Choice(&["point", "prim", "detail"]),
                Check::Any,
                "Destination domain (`point`, `prim`, `detail`).",
            ),
            param(
                "mode",
                ParamKind::Choice(&["average", "min", "max", "sum"]),
                Check::Any,
                "Reduction mode (`average`, `min`, `max`, `sum`).",
            ),
        ],
    )
    .with_rule(domains_differ),
    NodeDef::new(
        "blast",
        "modifier",
        "Deletes primitives or points matching a named group or attribute mask, compacting topology.",
        &[input("input", "Upstream geometry stream.")],
        &[
            param(
                "group_name",
                ParamKind::Text,
                Check::Identifier,
                "Name of the primitive/point group or mask attribute.",
            ),
            param(
                "negate",
                ParamKind::Bool,
                Check::Any,
                "When true, deletes non-selected elements instead of selected ones.",
            ),
        ],
    ),
    NodeDef::new(
        "uvproject",
        "uv",
        "Projects normalized point UV coordinates (`v@uv` in [0, 1]^2) using planar, cylindrical, spherical, or box projection.",
        &[input("in", "Upstream geometry stream.")],
        &[param(
            "mode",
            ParamKind::Choice(&[
                "planar_y",
                "planar_z",
                "planar_x",
                "cylindrical",
                "spherical",
                "box",
            ]),
            Check::Any,
            "Projection mode (`planar_y`, `planar_z`, `planar_x`, `cylindrical`, `spherical`, `box`).",
        )],
    ),
    NodeDef::new(
        "heightfield",
        "terrain",
        "Generates a 2D Copy-on-Write tiled heightfield terrain grid with `height`, `mask`, `water`, and `sediment` layers.",
        &[],
        &[
            param(
                "rows",
                ParamKind::U32,
                Check::UintAtLeast(2),
                "Number of grid rows along +Z (>= 2).",
            ),
            param(
                "cols",
                ParamKind::U32,
                Check::UintAtLeast(2),
                "Number of grid columns along +X (>= 2).",
            ),
            param(
                "size_x",
                ParamKind::F32,
                Check::Positive,
                "Total terrain extent along +X in metres (> 0).",
            ),
            param(
                "size_z",
                ParamKind::F32,
                Check::Positive,
                "Total terrain extent along +Z in metres (> 0).",
            ),
        ],
    ),
    NodeDef::new(
        "hf_noise",
        "terrain",
        "Applies deterministic derivative-warped multi-fractal fBm noise (`Philox4x32`) to a heightfield terrain.",
        &[input(
            "in",
            "Upstream heightfield terrain or grid geometry.",
        )],
        &[
            param(
                "amplitude",
                ParamKind::F32,
                Check::Any,
                "Vertical displacement amplitude in metres.",
            ),
            param(
                "frequency",
                ParamKind::F32,
                Check::Positive,
                "Base spatial frequency (> 0).",
            ),
            param(
                "octaves",
                ParamKind::U32,
                Check::UintRange(1, 12),
                "Number of fBm octaves (1..=12).",
            ),
            param(
                "seed",
                ParamKind::U64,
                Check::Any,
                "Deterministic Philox4x32 PRNG seed.",
            ),
        ],
    ),
    NodeDef::new(
        "hf_erode",
        "terrain",
        "Simulates shallow-water virtual-pipe hydraulic erosion and thermal talus slumping across a heightfield terrain.",
        &[input("in", "Upstream heightfield terrain geometry.")],
        &[
            param(
                "iterations",
                ParamKind::U32,
                Check::UintRange(1, 500),
                "Number of hydraulic/thermal solver steps (1..=500).",
            ),
            param(
                "rain_rate",
                ParamKind::F32,
                Check::FloatAtLeast(0.0),
                "Rainfall rate per unit step (>= 0).",
            ),
            param(
                "talus_angle",
                ParamKind::F32,
                Check::FloatRange(1.0, 89.0),
                "Critical thermal talus angle in degrees (1..=89).",
            ),
            param(
                "seed",
                ParamKind::U64,
                Check::Any,
                "Deterministic Philox4x32 rainfall seed.",
            ),
        ],
    ),
    NodeDef::new(
        "vdb_from_polygons",
        "vdb",
        "Voxelizes a polygon mesh into a narrow-band sparse 8x8x8 brick signed distance field (`VdbGrid`) using generalized solid-angle winding numbers.",
        &[input("in", "Upstream polygon mesh.")],
        &[param(
            "voxel_size",
            ParamKind::F32,
            Check::FloatAtLeast(0.05),
            "World-space voxel edge length in metres (>= 0.05).",
        )],
    ),
    NodeDef::new(
        "vdb_reshape",
        "vdb",
        "Applies morphological level-set dilation/erosion (`offset`) and Laplacian smoothing to a narrow-band SDF.",
        &[input("in", "Upstream VDB signed distance field.")],
        &[
            param(
                "offset",
                ParamKind::F32,
                Check::Any,
                "Signed world-space level-set offset (> 0 dilates, < 0 erodes).",
            ),
            param(
                "smooth_iterations",
                ParamKind::U32,
                Check::UintRange(0, 50),
                "Number of 6-neighbor Laplacian smoothing passes (0..=50).",
            ),
        ],
    ),
    NodeDef::new(
        "convert_vdb",
        "vdb",
        "Extracts the zero-isosurface of a VDB signed distance field as a guaranteed closed manifold all-quad mesh via Dual Surface Nets.",
        &[input("in", "Upstream VDB signed distance field.")],
        &[],
    ),
    NodeDef::new(
        "boolean",
        "modeling",
        "Performs a guaranteed-manifold volumetric CSG boolean (`union`, `intersect`, `subtract`) between two polygon meshes.",
        &[
            input("left", "Primary A polygon mesh."),
            input("right", "Secondary B polygon mesh."),
        ],
        &[
            param(
                "mode",
                ParamKind::Choice(&["union", "intersect", "subtract"]),
                Check::Any,
                "Boolean operation (`union`, `intersect`, `subtract`).",
            ),
            param(
                "voxel_size",
                ParamKind::F32,
                Check::FloatAtLeast(0.05),
                "World-space SDF evaluation voxel size (>= 0.05).",
            ),
        ],
    ),
    NodeDef::new(
        "scatter",
        "generator_modifier",
        "Deterministically scatters points across the input bounding volume using Philox4x32.",
        &[input(
            "input",
            "Upstream geometry defining the bounding region.",
        )],
        &[
            param(
                "count",
                ParamKind::U32,
                Check::Any,
                "Number of points to scatter.",
            ),
            param(
                "seed",
                ParamKind::U64,
                Check::Any,
                "Deterministic 64-bit Philox seed.",
            ),
        ],
    )
    .seeded_by_node_id(),
    NodeDef::new(
        "copytopoints",
        "instancer",
        "Copies the source geometry onto every point of the target geometry (scaling by `f@pscale` if present).",
        &[
            input("source", "Prototype geometry to copy."),
            input("target", "Target point cloud onto which copies are placed."),
        ],
        &[],
    ),
    NodeDef::new(
        "merge",
        "utility",
        "Concatenates two upstream geometry streams in deterministic left-then-right order.",
        &[
            input("left", "First upstream geometry stream."),
            input("right", "Second upstream geometry stream."),
        ],
        &[],
    ),
    NodeDef::new(
        "asset",
        "generator",
        "Outputs imported geometry by its content hash, as returned by `geo_import`. The scene does not depend on the source file afterwards.",
        &[],
        &[param(
            "content_hash",
            ParamKind::Text,
            Check::ContentHash,
            "BLAKE3 content hash: 64 lowercase hexadecimal digits.",
        )],
    ),
    NodeDef::new(
        "sphere",
        "generator",
        "Generates a closed, outward-facing latitude-longitude sphere centred on the origin.",
        &[],
        &[
            param(
                "radius",
                ParamKind::F32,
                Check::Positive,
                "Sphere radius in metres (> 0).",
            ),
            param(
                "rows",
                ParamKind::U32,
                Check::UintAtLeast(2),
                "Latitude bands (>= 2).",
            ),
            param(
                "cols",
                ParamKind::U32,
                Check::UintAtLeast(3),
                "Longitude segments (>= 3).",
            ),
        ],
    ),
    NodeDef::new(
        "tube",
        "generator",
        "Generates a closed, capped cylinder centred on the origin along +Y.",
        &[],
        &[
            param(
                "radius",
                ParamKind::F32,
                Check::Positive,
                "Tube radius in metres (> 0).",
            ),
            param(
                "height",
                ParamKind::F32,
                Check::Positive,
                "Tube height in metres (> 0).",
            ),
            param(
                "rows",
                ParamKind::U32,
                Check::UintAtLeast(1),
                "Height bands (>= 1).",
            ),
            param(
                "cols",
                ParamKind::U32,
                Check::UintAtLeast(3),
                "Sides (>= 3).",
            ),
        ],
    ),
    NodeDef::new(
        "torus",
        "generator",
        "Generates a closed quad torus around +Y.",
        &[],
        &[
            param(
                "major_radius",
                ParamKind::F32,
                Check::Positive,
                "Distance from the axis to the tube centre (> minor_radius).",
            ),
            param(
                "minor_radius",
                ParamKind::F32,
                Check::Positive,
                "Tube radius (> 0).",
            ),
            param(
                "rows",
                ParamKind::U32,
                Check::UintAtLeast(3),
                "Segments around the axis (>= 3).",
            ),
            param(
                "cols",
                ParamKind::U32,
                Check::UintAtLeast(3),
                "Segments around the tube (>= 3).",
            ),
        ],
    )
    .with_rule(major_exceeds_minor),
    NodeDef::new(
        "line",
        "generator",
        "Generates one straight polyline of evenly spaced points.",
        &[],
        &[
            param("start", ParamKind::Vec3, Check::Any, "First point."),
            param("end", ParamKind::Vec3, Check::Any, "Last point."),
            param(
                "points",
                ParamKind::U32,
                Check::UintAtLeast(2),
                "Point count (>= 2).",
            ),
        ],
    ),
    NodeDef::new(
        "xform",
        "transform",
        "Applies scale, Euler XYZ rotation (degrees), then translation to positions and `N`; a mirroring scale also reverses winding so solids keep facing outward.",
        &[input("input", "Upstream geometry.")],
        &[
            param(
                "translate",
                ParamKind::Vec3,
                Check::Any,
                "Translation in metres.",
            ),
            param(
                "rotate_degrees",
                ParamKind::Vec3,
                Check::Any,
                "Euler rotation, applied as Rz * Ry * Rx.",
            ),
            param(
                "scale",
                ParamKind::Vec3,
                Check::VecNoZeroComponent,
                "Per-axis scale (all non-zero).",
            ),
        ],
    ),
    NodeDef::new(
        "reverse",
        "modeling",
        "Flips the orientation of every polygon, keeping attributes attached.",
        &[input("input", "Upstream geometry.")],
        &[],
    ),
    NodeDef::new(
        "triangulate",
        "modeling",
        "Splits polygons into triangles, handling concave outlines. Point data survives; vertex and primitive attributes do not.",
        &[input("input", "Upstream geometry.")],
        &[],
    ),
    NodeDef::new(
        "fuse",
        "modeling",
        "Welds points closer than a distance, removing collapsed primitives. Point attributes of the lowest-indexed point survive; primitive attributes do not.",
        &[input("input", "Upstream geometry.")],
        &[param(
            "distance",
            ParamKind::F32,
            Check::FloatAtLeast(0.0),
            "Weld distance in metres (>= 0).",
        )],
    ),
    NodeDef::new(
        "group_bounds",
        "attributes",
        "Creates a named point or primitive group from an axis-aligned box, for use by `blast`.",
        &[input("input", "Upstream geometry.")],
        &[
            param(
                "group_name",
                ParamKind::Text,
                Check::Identifier,
                "Group name.",
            ),
            param(
                "domain",
                ParamKind::Choice(&["point", "prim", "detail"]),
                Check::Any,
                "`point` or `prim`.",
            ),
            param("min", ParamKind::Vec3, Check::Any, "Box minimum corner."),
            param("max", ParamKind::Vec3, Check::Any, "Box maximum corner."),
        ],
    )
    .with_rule(bounds_are_ordered),
    NodeDef::new(
        "resample",
        "curves",
        "Rebuilds polylines with uniform arc-length spacing and exact endpoints. Polygons pass through; attributes do not survive.",
        &[input("input", "Upstream geometry.")],
        &[param(
            "segment_length",
            ParamKind::F32,
            Check::Positive,
            "Target spacing in metres (> 0).",
        )],
    ),
    NodeDef::new(
        "sweep",
        "curves",
        "Sweeps a regular polygon along polylines using rotation-minimizing frames, consuming polygons.",
        &[input("input", "Upstream curves.")],
        &[
            param(
                "radius",
                ParamKind::F32,
                Check::Positive,
                "Tube radius in metres (> 0).",
            ),
            param(
                "sides",
                ParamKind::U32,
                Check::UintAtLeast(3),
                "Cross-section sides (>= 3).",
            ),
            param(
                "capped",
                ParamKind::Bool,
                Check::Any,
                "Close tube ends with caps.",
            ),
        ],
    ),
    NodeDef::new(
        "attribtransfer",
        "attributes",
        "Transfers an f32 or vec3 point attribute from a source by normalized kernel average within a radius; points out of range keep their value.",
        &[
            input("input", "Geometry receiving the attribute."),
            input("source", "Geometry providing the attribute."),
        ],
        &[
            param(
                "name",
                ParamKind::Text,
                Check::Identifier,
                "Point attribute name.",
            ),
            param(
                "radius",
                ParamKind::F32,
                Check::Positive,
                "Kernel radius in metres (> 0).",
            ),
        ],
    ),
    NodeDef::new(
        "measure",
        "attributes",
        "Writes primitive `f@area` and `f@perimeter` and detail `f@total_area` and `f@total_volume`.",
        &[input("input", "Upstream geometry.")],
        &[],
    ),
    NodeDef::new(
        "polybevel",
        "modeling",
        "Bevels manifold edges at or above a dihedral angle by offsetting each adjacent face inward. Attributes do not survive.",
        &[input("input", "Upstream manifold polygons.")],
        &[
            param(
                "distance",
                ParamKind::F32,
                Check::Positive,
                "Inward face offset in metres (> 0).",
            ),
            param(
                "min_angle_degrees",
                ParamKind::F32,
                Check::FloatRange(0.0, 180.0),
                "Minimum dihedral angle to bevel, in [0, 180].",
            ),
        ],
    ),
    NodeDef::new(
        "polyreduce",
        "modeling",
        "Decimates to a target triangle ratio with quadric error metrics, preserving manifoldness, genus, and boundaries. Output carries positions only.",
        &[input("input", "Upstream oriented manifold polygons.")],
        &[param(
            "target_ratio",
            ParamKind::F32,
            Check::FloatOpenClosed(0.0, 1.0),
            "Fraction of triangles to keep, in (0, 1].",
        )],
    ),
    NodeDef::new(
        "remesh",
        "modeling",
        "Isotropically remeshes to a target edge length, keeping boundaries and sharp features in place. Output carries positions only.",
        &[input("input", "Upstream oriented manifold polygons.")],
        &[
            param(
                "target_length",
                ParamKind::F32,
                Check::Positive,
                "Target edge length in metres (> 0).",
            ),
            param(
                "iterations",
                ParamKind::U32,
                Check::UintRange(1, 100),
                "Refinement rounds, in [1, 100].",
            ),
            param(
                "feature_angle_degrees",
                ParamKind::F32,
                Check::FloatRange(0.0, 180.0),
                "Dihedral angle at or above which edges stay sharp, in [0, 180].",
            ),
        ],
    ),
    NodeDef::new(
        "clip",
        "modeling",
        "Removes geometry on the +normal side of a plane. With caps, closed solids stay closed. Output carries positions only.",
        &[input("input", "Upstream polygons.")],
        &[
            param("origin", ParamKind::Vec3, Check::Any, "Point on the plane."),
            param(
                "normal",
                ParamKind::Vec3,
                Check::VecNonZero,
                "Plane normal pointing at the removed side (non-zero).",
            ),
            param(
                "cap",
                ParamKind::Bool,
                Check::Any,
                "Close openings on the plane.",
            ),
        ],
    ),
    NodeDef::new(
        "voronoifracture",
        "fracture",
        "Cuts a closed manifold into the Voronoi cells of seed points; each piece is a closed shell tagged `i@piece` on points and primitives, conserving total volume.",
        &[
            input("input", "Closed manifold solid."),
            input("seeds", "Points used as Voronoi seeds."),
        ],
        &[],
    ),
    NodeDef::new(
        "scatter_surface",
        "generator_modifier",
        "Scatters points on polygon surfaces with area-uniform density, writing `v@N`; a positive minimum distance keeps only samples at least that far apart.",
        &[input("input", "Upstream polygons.")],
        &[
            param(
                "count",
                ParamKind::U32,
                Check::Any,
                "Maximum number of points.",
            ),
            param(
                "seed",
                ParamKind::U64,
                Check::Any,
                "Deterministic 64-bit Philox seed.",
            ),
            param(
                "min_distance",
                ParamKind::F32,
                Check::FloatAtLeast(0.0),
                "Minimum pairwise spacing in metres (>= 0; 0 disables).",
            ),
        ],
    ),
    NodeDef::new(
        "uvflatten",
        "uv",
        "Flattens polygons with least-squares conformal maps into non-overlapping islands in the unit square, cutting closed shapes into disks. Writes vertex `v@uv` and primitive `i@uv_island` with no inverted triangles.",
        &[input("input", "Upstream polygons.")],
        &[param(
            "seam_angle_degrees",
            ParamKind::F32,
            Check::FloatRange(0.0, 180.0),
            "Dihedral angle above which faces start in separate islands, in [0, 180].",
        )],
    ),
    NodeDef::new(
        "uvlayout",
        "uv",
        "Packs existing `v@uv` islands into UDIM tiles (ten per row) at one uniform scale.",
        &[input("input", "Geometry with vertex or point `v@uv`.")],
        &[
            param(
                "padding",
                ParamKind::F32,
                Check::FloatClosedOpen(0.0, 0.25),
                "Space around each island, in [0, 0.25).",
            ),
            param(
                "tiles",
                ParamKind::U32,
                Check::UintRange(1, 100),
                "Tile count, in [1, 100].",
            ),
        ],
    ),
];
