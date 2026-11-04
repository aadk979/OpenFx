//! Immutable acyclic procedural node graph, canonical `.ofxs` serialization,
//! fine-grained graph edits, and operator catalog introspection.
use pcc_id::{NodeId, RevisionId};
use pcc_math::Vec3;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod nodes;
mod zones;

use nodes::parse_u64_strict;

pub use zones::{FOREACH_OP, FOREACH_PIECE_OP, SOLVER_OP, STATE_PREV_OP, StateZone, ZoneKind};

pub use nodes::{
    Check, Node, NodeDef, NodeInput, NodeParam, NodeRegistrationError, ParamKind, ParamValue,
    node_catalog, node_def, register_node_def,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttrDomain {
    Point,
    Prim,
    Detail,
}

impl AttrDomain {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Point => "point",
            Self::Prim => "prim",
            Self::Detail => "detail",
        }
    }

    pub fn from_token(token: &str) -> Result<Self, SceneError> {
        match token {
            "point" => Ok(Self::Point),
            "prim" => Ok(Self::Prim),
            "detail" => Ok(Self::Detail),
            _ => Err(SceneError::Field),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromoteMode {
    Average,
    Min,
    Max,
    Sum,
}

impl PromoteMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Average => "average",
            Self::Min => "min",
            Self::Max => "max",
            Self::Sum => "sum",
        }
    }

    pub fn from_token(token: &str) -> Result<Self, SceneError> {
        match token {
            "average" => Ok(Self::Average),
            "min" => Ok(Self::Min),
            "max" => Ok(Self::Max),
            "sum" => Ok(Self::Sum),
            _ => Err(SceneError::Field),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UvProjectMode {
    PlanarY,
    PlanarZ,
    PlanarX,
    Cylindrical,
    Spherical,
    Box,
}

impl UvProjectMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlanarY => "planar_y",
            Self::PlanarZ => "planar_z",
            Self::PlanarX => "planar_x",
            Self::Cylindrical => "cylindrical",
            Self::Spherical => "spherical",
            Self::Box => "box",
        }
    }

    pub fn from_token(token: &str) -> Result<Self, SceneError> {
        match token {
            "planar_y" => Ok(Self::PlanarY),
            "planar_z" => Ok(Self::PlanarZ),
            "planar_x" => Ok(Self::PlanarX),
            "cylindrical" => Ok(Self::Cylindrical),
            "spherical" => Ok(Self::Spherical),
            "box" => Ok(Self::Box),
            _ => Err(SceneError::Field),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BooleanMode {
    Union,
    Intersect,
    Subtract,
}

impl BooleanMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Union => "union",
            Self::Intersect => "intersect",
            Self::Subtract => "subtract",
        }
    }

    pub fn from_token(token: &str) -> Result<Self, SceneError> {
        match token {
            "union" => Ok(Self::Union),
            "intersect" => Ok(Self::Intersect),
            "subtract" => Ok(Self::Subtract),
            _ => Err(SceneError::Field),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "edit", rename_all = "snake_case")]
pub enum GraphEdit {
    AddNode {
        node: Node,
    },
    RemoveNode {
        id: NodeId,
    },
    /// Sets one parameter. Numbers are coerced to the parameter's declared kind.
    SetParam {
        id: NodeId,
        name: String,
        value: ParamValue,
    },
    /// Connects one input port, by position, to another node's output.
    SetInput {
        id: NodeId,
        slot: usize,
        input: NodeId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodePortSpec {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeParamSpec {
    pub name: String,
    pub param_type: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeTypeSpec {
    pub type_name: String,
    pub category: String,
    pub summary: String,
    pub inputs: Vec<NodePortSpec>,
    pub outputs: Vec<NodePortSpec>,
    pub params: Vec<NodeParamSpec>,
}

#[must_use]
pub fn search_node_catalog(query: &str) -> Vec<NodeTypeSpec> {
    let needle = query.trim().to_ascii_lowercase();
    node_catalog()
        .into_iter()
        .filter(|spec| {
            needle.is_empty()
                || spec.type_name.to_ascii_lowercase().contains(&needle)
                || spec.category.to_ascii_lowercase().contains(&needle)
                || spec.summary.to_ascii_lowercase().contains(&needle)
        })
        .collect()
}

#[must_use]
pub fn inspect_node_type(type_name: &str) -> Option<NodeTypeSpec> {
    let normalized = type_name.trim().to_ascii_lowercase();
    node_catalog()
        .into_iter()
        .find(|spec| spec.type_name == normalized)
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphSnapshot {
    pub revision: RevisionId,
    pub nodes: Vec<Node>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneError {
    Header,
    Field,
    NodeOrder,
    NonFinite,
}

impl GraphSnapshot {
    #[must_use]
    pub fn demo(count: u32, offset: Vec3) -> Self {
        Self {
            revision: RevisionId(1),
            nodes: vec![
                Node::new("create", NodeId(1)).with_u32("count", count),
                Node::new("transform", NodeId(2))
                    .with_input(NodeId(1))
                    .with_vec3("offset", offset),
            ],
        }
    }

    /// Validates topology, node IDs, and finite parameter invariants of the snapshot.
    pub fn validate(&self) -> Result<(), SceneError> {
        if self.revision.0 == 0 {
            return Err(SceneError::Field);
        }
        if self.nodes.len() < 2 {
            return Err(SceneError::NodeOrder);
        }
        if !self.nodes.first().is_some_and(Node::is_generator) {
            return Err(SceneError::NodeOrder);
        }
        if self.nodes.last().is_some_and(Node::is_generator) {
            return Err(SceneError::NodeOrder);
        }
        let mut defined = BTreeSet::new();
        for node in &self.nodes {
            let id = node.id();
            if id.0 == 0 {
                return Err(SceneError::Field);
            }
            if defined.contains(&id) {
                return Err(SceneError::NodeOrder);
            }
            for dependency in node.dependencies() {
                if !defined.contains(&dependency) {
                    return Err(SceneError::NodeOrder);
                }
            }
            node_def(&node.op)
                .ok_or(SceneError::Field)?
                .validate(node)?;
            defined.insert(id);
        }
        for (entry, source) in zones::entry_source_edges(&self.nodes) {
            let position = |id: NodeId| self.nodes.iter().position(|node| node.id == id);
            if position(source) > position(entry) {
                return Err(SceneError::NodeOrder);
            }
        }
        self.zones()?;
        Ok(())
    }

    /// Applies a batch of fine-grained edits, topologically sorts the resulting node graph,
    /// and validates all DAG and numeric invariants before returning the new snapshot.
    pub fn apply_edits(
        &self,
        next_revision: RevisionId,
        edits: &[GraphEdit],
    ) -> Result<Self, SceneError> {
        if edits.is_empty() {
            return Err(SceneError::Field);
        }
        let mut nodes = self.nodes.clone();
        for edit in edits {
            match edit {
                GraphEdit::AddNode { node } => {
                    let id = node.id();
                    if id.0 == 0 {
                        return Err(SceneError::Field);
                    }
                    if nodes.iter().any(|existing| existing.id() == id) {
                        return Err(SceneError::NodeOrder);
                    }
                    let mut added = node.clone();
                    added.canonicalize_numbers();
                    nodes.push(added);
                }
                GraphEdit::RemoveNode { id } => {
                    let before = nodes.len();
                    nodes.retain(|node| node.id() != *id);
                    if nodes.len() == before {
                        return Err(SceneError::Field);
                    }
                }
                GraphEdit::SetParam { id, name, value } => {
                    let node = nodes
                        .iter_mut()
                        .find(|node| node.id() == *id)
                        .ok_or(SceneError::Field)?;
                    if !node.params.contains_key(name) {
                        return Err(SceneError::Field);
                    }
                    node.params.insert(name.clone(), value.clone());
                    node.canonicalize_numbers();
                }
                GraphEdit::SetInput { id, slot, input } => {
                    let node = nodes
                        .iter_mut()
                        .find(|node| node.id() == *id)
                        .ok_or(SceneError::Field)?;
                    *node.inputs.get_mut(*slot).ok_or(SceneError::Field)? = *input;
                }
            }
        }
        let sorted = topological_sort(&nodes)?;
        let candidate = Self {
            revision: next_revision,
            nodes: sorted,
        };
        candidate.validate()?;
        Ok(candidate)
    }

    /// Canonical `.ofxs` serialization for the DAG snapshot.
    #[must_use]
    pub fn to_ofxs(&self) -> String {
        let mut text = format!("ofxs 1\nrevision {}\n", self.revision.0);
        for node in &self.nodes {
            node.write_ofxs_line(&mut text);
        }
        text
    }

    pub fn from_ofxs(text: &str) -> Result<Self, SceneError> {
        if !text.is_ascii() || text.contains('\r') {
            return Err(SceneError::Field);
        }
        let Some(body) = text.strip_suffix('\n') else {
            return Err(SceneError::Field);
        };
        let mut lines = body.split('\n');
        if lines.next() != Some("ofxs 1") {
            return Err(SceneError::Header);
        }
        let revision = parse_u64_strict(parse_pair(lines.next(), "revision")?)?;
        let mut nodes = Vec::new();
        for line in lines {
            if line.is_empty() {
                return Err(SceneError::Field);
            }
            nodes.push(Node::parse_ofxs_line(line)?);
        }
        if nodes.len() < 2 {
            return Err(SceneError::Field);
        }
        let snapshot = Self {
            revision: RevisionId(revision),
            nodes,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
}

fn topological_sort(nodes: &[Node]) -> Result<Vec<Node>, SceneError> {
    if nodes.len() < 2 {
        return Err(SceneError::NodeOrder);
    }
    let mut index_by_id = BTreeMap::new();
    for (idx, node) in nodes.iter().enumerate() {
        let id = node.id();
        if id.0 == 0 {
            return Err(SceneError::Field);
        }
        if index_by_id.insert(id, idx).is_some() {
            return Err(SceneError::NodeOrder);
        }
    }
    let mut in_degree = vec![0usize; nodes.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (idx, node) in nodes.iter().enumerate() {
        for input_id in node.dependencies() {
            let Some(&upstream_idx) = index_by_id.get(&input_id) else {
                return Err(SceneError::NodeOrder);
            };
            if upstream_idx == idx {
                return Err(SceneError::NodeOrder);
            }
            dependents[upstream_idx].push(idx);
            in_degree[idx] += 1;
        }
    }
    for (entry, source) in zones::entry_source_edges(nodes) {
        let Some((&entry_idx, &source_idx)) = index_by_id.get(&entry).zip(index_by_id.get(&source))
        else {
            return Err(SceneError::NodeOrder);
        };
        dependents[source_idx].push(entry_idx);
        in_degree[entry_idx] += 1;
    }
    let mut ready: Vec<usize> = in_degree
        .iter()
        .enumerate()
        .filter_map(|(idx, &deg)| (deg == 0).then_some(idx))
        .collect();
    let mut result = Vec::with_capacity(nodes.len());
    while !ready.is_empty() {
        let current = ready.remove(0);
        result.push(nodes[current].clone());
        for &dep in &dependents[current] {
            in_degree[dep] -= 1;
            if in_degree[dep] == 0 {
                let pos = ready.partition_point(|&existing| existing < dep);
                ready.insert(pos, dep);
            }
        }
    }
    if result.len() != nodes.len() {
        return Err(SceneError::NodeOrder);
    }
    Ok(result)
}

fn parse_pair<'a>(line: Option<&'a str>, key: &str) -> Result<&'a str, SceneError> {
    let line = line.ok_or(SceneError::Field)?;
    line.strip_prefix(&format!("{key} "))
        .filter(|s| !s.is_empty())
        .ok_or(SceneError::Field)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SplitMix64 {
        state: u64,
    }

    impl SplitMix64 {
        const fn new(seed: u64) -> Self {
            Self { state: seed }
        }

        fn next_u64(&mut self) -> u64 {
            self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn next_u32(&mut self) -> u32 {
            (self.next_u64() >> 32) as u32
        }

        fn next_finite_f32(&mut self) -> f32 {
            loop {
                let bits = self.next_u32();
                let candidate = f32::from_bits(bits);
                if candidate.is_finite() {
                    return if candidate == 0.0 { 0.0 } else { candidate };
                }
            }
        }
    }

    #[test]
    fn ofxs_is_canonical_and_round_trips() {
        let graph = GraphSnapshot::demo(100, Vec3::checked(1., 2., 3.).unwrap_or(Vec3::ZERO));
        let saved = graph.to_ofxs();
        assert_eq!(
            GraphSnapshot::from_ofxs(&saved).map(|value| value.to_ofxs()),
            Ok(saved)
        );
    }

    #[test]
    fn ofxs_rejects_non_finite_values() {
        assert!(matches!(
            GraphSnapshot::from_ofxs("ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 NaN 0e0 0e0\n"),
            Err(SceneError::NonFinite)
        ));
        assert!(matches!(
            GraphSnapshot::from_ofxs("ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 inf 0e0 0e0\n"),
            Err(SceneError::NonFinite)
        ));
        assert!(matches!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 -inf 0e0\n"
            ),
            Err(SceneError::NonFinite)
        ));
        assert!(matches!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 1e50 0e0 0e0\n"
            ),
            Err(SceneError::NonFinite)
        ));
    }

    #[test]
    fn property_valid_graphs_round_trip_canonically() {
        let mut rng = SplitMix64::new(0x0FF5_EC0D_E123_4567);
        for _ in 0..1_000 {
            let revision = (rng.next_u64() % 1_000_000).saturating_add(1);
            let create_id = (rng.next_u64() % 10_000).saturating_add(1);
            let mut transform_id = (rng.next_u64() % 10_000).saturating_add(1);
            if transform_id == create_id {
                transform_id = transform_id.saturating_add(1);
            }
            let count = rng.next_u32() % 500_000;
            let x = rng.next_finite_f32();
            let y = rng.next_finite_f32();
            let z = rng.next_finite_f32();
            let offset = Vec3::checked(x, y, z).unwrap_or(Vec3::ZERO);

            let snapshot = GraphSnapshot {
                revision: RevisionId(revision),
                nodes: vec![
                    Node::new("create", NodeId(create_id)).with_u32("count", count),
                    Node::new("transform", NodeId(transform_id))
                        .with_input(NodeId(create_id))
                        .with_vec3("offset", offset),
                ],
            };
            assert_eq!(snapshot.validate(), Ok(()));

            let serialized = snapshot.to_ofxs();
            let parsed = GraphSnapshot::from_ofxs(&serialized);
            assert_eq!(parsed.as_ref(), Ok(&snapshot));
            assert_eq!(parsed.map(|g| g.to_ofxs()), Ok(serialized));
        }
    }

    #[test]
    fn graph_validation_rejects_malformed_topologies_and_non_finite_offsets() {
        let empty = GraphSnapshot {
            revision: RevisionId(1),
            nodes: Vec::new(),
        };
        assert_eq!(empty.validate(), Err(SceneError::NodeOrder));

        let zero_rev = GraphSnapshot::demo(4, Vec3::ZERO);
        let zero_rev = GraphSnapshot {
            revision: RevisionId(0),
            nodes: zero_rev.nodes,
        };
        assert_eq!(zero_rev.validate(), Err(SceneError::Field));

        let duplicate_id = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::new("create", NodeId(1)).with_u32("count", 4),
                Node::new("transform", NodeId(1))
                    .with_input(NodeId(1))
                    .with_vec3("offset", Vec3::ZERO),
            ],
        };
        assert_eq!(duplicate_id.validate(), Err(SceneError::NodeOrder));

        let dangling_input = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::new("create", NodeId(1)).with_u32("count", 4),
                Node::new("transform", NodeId(2))
                    .with_input(NodeId(99))
                    .with_vec3("offset", Vec3::ZERO),
            ],
        };
        assert_eq!(dangling_input.validate(), Err(SceneError::NodeOrder));

        let reversed_order = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::new("transform", NodeId(2))
                    .with_input(NodeId(1))
                    .with_vec3("offset", Vec3::ZERO),
                Node::new("create", NodeId(1)).with_u32("count", 4),
            ],
        };
        assert_eq!(reversed_order.validate(), Err(SceneError::NodeOrder));

        let raw_nan_offset = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::new("create", NodeId(1)).with_u32("count", 4),
                Node::new("transform", NodeId(2))
                    .with_input(NodeId(1))
                    .with_vec3(
                        "offset",
                        Vec3 {
                            x: f32::NAN,
                            y: 0.0,
                            z: 0.0,
                        },
                    ),
            ],
        };
        assert_eq!(raw_nan_offset.validate(), Err(SceneError::NonFinite));
    }

    #[test]
    fn fuzz_malformed_ofxs_corpus_and_mutations_never_panic_and_reject_invalid() {
        let malformed_corpus: &[(&str, SceneError)] = &[
            ("", SceneError::Field),
            ("\n", SceneError::Header),
            (
                "ofxs 2\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Header,
            ),
            (
                "ofxs 1\r\nrevision 1\r\ncreate 1 1\r\ntransform 2 1 0e0 0e0 0e0\r\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 0\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision +1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision -1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 18446744073709551616\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 0 1\ntransform 2 0 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 4294967296\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate  1 1\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1 extra\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ntransform 2 1 0e0 0e0 0e0\ncreate 1 1\n",
                SceneError::NodeOrder,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 1 1 0e0 0e0 0e0\n",
                SceneError::NodeOrder,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 99 0e0 0e0 0e0\n",
                SceneError::NodeOrder,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\n\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\ntrailing\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 \u{00e9} 0e0 0e0\n",
                SceneError::Field,
            ),
        ];

        for (input, expected_err) in malformed_corpus {
            assert_eq!(
                GraphSnapshot::from_ofxs(input),
                Err(*expected_err),
                "unexpected result for corpus input: {input:?}"
            );
        }

        // Deterministic byte-level fuzzing around a valid canonical `.ofxs` payload.
        let base = GraphSnapshot::demo(256, Vec3::checked(1.25, -3.5, 42.0).unwrap_or(Vec3::ZERO))
            .to_ofxs();
        let base_bytes = base.as_bytes();
        let mut rng = SplitMix64::new(0xF022_CAFE_BAAD_BEEF);

        for _ in 0..1_000 {
            let mut mutated = base_bytes.to_vec();
            let op = rng.next_u32() % 4;
            match op {
                0 => {
                    let idx = (rng.next_u64() as usize) % mutated.len();
                    mutated[idx] ^= ((rng.next_u32() % 255) as u8).saturating_add(1);
                }
                1 => {
                    let idx = (rng.next_u64() as usize) % mutated.len();
                    mutated.remove(idx);
                }
                2 => {
                    let idx = (rng.next_u64() as usize) % (mutated.len() + 1);
                    let byte = (rng.next_u32() & 0x7F) as u8;
                    mutated.insert(idx, byte);
                }
                _ => {
                    let cut = (rng.next_u64() as usize) % mutated.len();
                    mutated.truncate(cut);
                }
            }
            if let Ok(as_str) = std::str::from_utf8(&mutated)
                && let Ok(parsed) = GraphSnapshot::from_ofxs(as_str)
            {
                assert_eq!(parsed.validate(), Ok(()));
                let round_tripped = parsed.to_ofxs();
                assert_eq!(GraphSnapshot::from_ofxs(&round_tripped), Ok(parsed));
            }
        }
    }

    fn v3(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    /// One valid instance of every SOP operator, wired over two generators.
    fn sop_operator_graph() -> GraphSnapshot {
        let sphere = NodeId(1);
        let seeds = NodeId(2);
        let curve = NodeId(3);
        let nodes = vec![
            Node::new("sphere", sphere)
                .with_f32("radius", 1.5)
                .with_u32("rows", 8)
                .with_u32("cols", 12),
            Node::new("tube", seeds)
                .with_f32("radius", 0.5)
                .with_f32("height", 2.0)
                .with_u32("rows", 2)
                .with_u32("cols", 8),
            Node::new("line", curve)
                .with_vec3("start", v3(0.0, 0.0, 0.0))
                .with_vec3("end", v3(1.0, 2.0, -3.5))
                .with_u32("points", 9),
            Node::new("torus", NodeId(4))
                .with_f32("major_radius", 2.0)
                .with_f32("minor_radius", 0.5)
                .with_u32("rows", 8)
                .with_u32("cols", 6),
            Node::new("xform", NodeId(5))
                .with_input(sphere)
                .with_vec3("translate", v3(1.0, 0.0, 0.0))
                .with_vec3("rotate_degrees", v3(0.0, 45.0, 0.0))
                .with_vec3("scale", v3(1.0, -2.0, 1.0)),
            Node::new("reverse", NodeId(6)).with_input(sphere),
            Node::new("triangulate", NodeId(7)).with_input(sphere),
            Node::new("fuse", NodeId(8))
                .with_input(sphere)
                .with_f32("distance", 0.001),
            Node::new("group_bounds", NodeId(9))
                .with_input(sphere)
                .with_text("group_name", "top".to_owned())
                .with_text("domain", AttrDomain::Prim.as_str())
                .with_vec3("min", v3(-2.0, 0.0, -2.0))
                .with_vec3("max", v3(2.0, 2.0, 2.0)),
            Node::new("resample", NodeId(10))
                .with_input(curve)
                .with_f32("segment_length", 0.25),
            Node::new("sweep", NodeId(11))
                .with_input(curve)
                .with_f32("radius", 0.1)
                .with_u32("sides", 6)
                .with_bool("capped", true),
            Node::new("attribtransfer", NodeId(12))
                .with_input(sphere)
                .with_input(seeds)
                .with_text("name", "heat".to_owned())
                .with_f32("radius", 0.5),
            Node::new("measure", NodeId(13)).with_input(sphere),
            Node::new("polybevel", NodeId(14))
                .with_input(sphere)
                .with_f32("distance", 0.01)
                .with_f32("min_angle_degrees", 30.0),
            Node::new("polyreduce", NodeId(15))
                .with_input(sphere)
                .with_f32("target_ratio", 0.5),
            Node::new("remesh", NodeId(16))
                .with_input(sphere)
                .with_f32("target_length", 0.3)
                .with_u32("iterations", 3)
                .with_f32("feature_angle_degrees", 60.0),
            Node::new("clip", NodeId(17))
                .with_input(sphere)
                .with_vec3("origin", v3(0.0, 0.25, 0.0))
                .with_vec3("normal", v3(0.0, 1.0, 0.0))
                .with_bool("cap", false),
            Node::new("voronoifracture", NodeId(18))
                .with_input(sphere)
                .with_input(seeds),
            Node::new("scatter_surface", NodeId(19))
                .with_input(sphere)
                .with_u32("count", 100)
                .with_u64("seed", u64::MAX)
                .with_f32("min_distance", 0.05),
            Node::new("uvflatten", NodeId(20))
                .with_input(sphere)
                .with_f32("seam_angle_degrees", 66.0),
            Node::new("uvlayout", NodeId(21))
                .with_input(NodeId(20))
                .with_f32("padding", 0.01)
                .with_u32("tiles", 4),
        ];
        GraphSnapshot {
            revision: RevisionId(7),
            nodes,
        }
    }

    #[test]
    fn every_sop_operator_round_trips_through_ofxs_and_is_in_the_catalog() {
        let graph = sop_operator_graph();
        assert!(graph.validate().is_ok());
        let text = graph.to_ofxs();
        let parsed = GraphSnapshot::from_ofxs(&text);
        assert_eq!(parsed.as_ref(), Ok(&graph));
        assert_eq!(parsed.map(|g| g.to_ofxs()), Ok(text.clone()));
        for line in text.lines().skip(2) {
            let token = line.split(' ').next().unwrap_or("");
            assert!(
                inspect_node_type(token).is_some(),
                "{token} is not in the catalog"
            );
        }
        // Every catalogued parameter and port appears as a serialized field of the node's line.
        for node in &graph.nodes {
            let line = text
                .lines()
                .skip(2)
                .find(|l| l.split(' ').nth(1) == Some(node.id().0.to_string().as_str()))
                .unwrap_or("");
            let token = line.split(' ').next().unwrap_or("");
            let spec = inspect_node_type(token);
            assert!(spec.is_some(), "{line}");
            if let Some(spec) = spec {
                let expected: usize = 2
                    + spec.inputs.len()
                    + spec
                        .params
                        .iter()
                        .map(|p| if p.param_type == "vec3" { 3 } else { 1 })
                        .sum::<usize>();
                assert_eq!(line.split(' ').count(), expected, "{line}");
            }
        }
    }

    #[test]
    fn sop_operator_parameters_are_validated() {
        let bad = |node: Node| {
            let mut nodes = vec![
                Node::new("sphere", NodeId(1))
                    .with_f32("radius", 1.0)
                    .with_u32("rows", 8)
                    .with_u32("cols", 8),
            ];
            nodes.push(node);
            // A scene may not end on a generator, so a consumer always follows the node under test.
            nodes.push(Node::new("reverse", NodeId(3)).with_input(NodeId(1)));
            GraphSnapshot {
                revision: RevisionId(1),
                nodes,
            }
            .validate()
        };
        let sphere = NodeId(1);
        let id = NodeId(2);
        let field = Err(SceneError::Field);
        let non_finite = Err(SceneError::NonFinite);
        let ones = v3(1.0, 1.0, 1.0);
        assert_eq!(
            bad(Node::new("sphere", id)
                .with_f32("radius", 0.0)
                .with_u32("rows", 8)
                .with_u32("cols", 8)),
            field
        );
        assert_eq!(
            bad(Node::new("sphere", id)
                .with_f32("radius", f32::NAN)
                .with_u32("rows", 8)
                .with_u32("cols", 8)),
            non_finite
        );
        assert_eq!(
            bad(Node::new("sphere", id)
                .with_f32("radius", 1.0)
                .with_u32("rows", 1)
                .with_u32("cols", 8)),
            field
        );
        assert_eq!(
            bad(Node::new("tube", id)
                .with_f32("radius", 1.0)
                .with_f32("height", -1.0)
                .with_u32("rows", 1)
                .with_u32("cols", 8)),
            field
        );
        assert_eq!(
            bad(Node::new("torus", id)
                .with_f32("major_radius", 0.5)
                .with_f32("minor_radius", 1.0)
                .with_u32("rows", 8)
                .with_u32("cols", 8)),
            field
        );
        assert_eq!(
            bad(Node::new("line", id)
                .with_vec3("start", ones)
                .with_vec3("end", ones)
                .with_u32("points", 1)),
            field
        );
        assert_eq!(
            bad(Node::new("xform", id)
                .with_input(sphere)
                .with_vec3("translate", ones)
                .with_vec3("rotate_degrees", ones)
                .with_vec3("scale", v3(0.0, 1.0, 1.0))),
            field
        );
        assert_eq!(
            bad(Node::new("xform", id)
                .with_input(sphere)
                .with_vec3("translate", v3(f32::INFINITY, 0.0, 0.0))
                .with_vec3("rotate_degrees", ones)
                .with_vec3("scale", ones)),
            non_finite
        );
        assert_eq!(
            bad(Node::new("fuse", id)
                .with_input(sphere)
                .with_f32("distance", -1.0)),
            field
        );
        assert_eq!(
            bad(Node::new("group_bounds", id)
                .with_input(sphere)
                .with_text("group_name", "g".to_owned())
                .with_text("domain", AttrDomain::Detail.as_str())
                .with_vec3("min", ones)
                .with_vec3("max", ones)),
            field
        );
        assert_eq!(
            bad(Node::new("group_bounds", id)
                .with_input(sphere)
                .with_text("group_name", "g".to_owned())
                .with_text("domain", AttrDomain::Point.as_str())
                .with_vec3("min", v3(2.0, 2.0, 2.0))
                .with_vec3("max", ones)),
            field
        );
        assert_eq!(
            bad(Node::new("group_bounds", id)
                .with_input(sphere)
                .with_text("group_name", "not valid".to_owned())
                .with_text("domain", AttrDomain::Point.as_str())
                .with_vec3("min", ones)
                .with_vec3("max", ones)),
            field
        );
        assert_eq!(
            bad(Node::new("resample", id)
                .with_input(sphere)
                .with_f32("segment_length", 0.0)),
            field
        );
        assert_eq!(
            bad(Node::new("sweep", id)
                .with_input(sphere)
                .with_f32("radius", 1.0)
                .with_u32("sides", 2)
                .with_bool("capped", false)),
            field
        );
        assert_eq!(
            bad(Node::new("attribtransfer", id)
                .with_input(sphere)
                .with_input(sphere)
                .with_text("name", "P".to_owned())
                .with_f32("radius", 1.0)),
            field
        );
        assert_eq!(
            bad(Node::new("polybevel", id)
                .with_input(sphere)
                .with_f32("distance", 0.1)
                .with_f32("min_angle_degrees", 181.0)),
            field
        );
        assert_eq!(
            bad(Node::new("polyreduce", id)
                .with_input(sphere)
                .with_f32("target_ratio", 0.0)),
            field
        );
        assert_eq!(
            bad(Node::new("polyreduce", id)
                .with_input(sphere)
                .with_f32("target_ratio", 1.5)),
            field
        );
        assert_eq!(
            bad(Node::new("remesh", id)
                .with_input(sphere)
                .with_f32("target_length", 1.0)
                .with_u32("iterations", 0)
                .with_f32("feature_angle_degrees", 30.0)),
            field
        );
        assert_eq!(
            bad(Node::new("remesh", id)
                .with_input(sphere)
                .with_f32("target_length", 1.0)
                .with_u32("iterations", 101)
                .with_f32("feature_angle_degrees", 30.0)),
            field
        );
        assert_eq!(
            bad(Node::new("clip", id)
                .with_input(sphere)
                .with_vec3("origin", ones)
                .with_vec3("normal", v3(0.0, 0.0, 0.0))
                .with_bool("cap", true)),
            field
        );
        assert_eq!(
            bad(Node::new("scatter_surface", id)
                .with_input(sphere)
                .with_u32("count", 1)
                .with_u64("seed", 1)
                .with_f32("min_distance", -0.5)),
            field
        );
        assert_eq!(
            bad(Node::new("uvflatten", id)
                .with_input(sphere)
                .with_f32("seam_angle_degrees", -1.0)),
            field
        );
        assert_eq!(
            bad(Node::new("uvlayout", id)
                .with_input(sphere)
                .with_f32("padding", 0.25)
                .with_u32("tiles", 1)),
            field
        );
        assert_eq!(
            bad(Node::new("uvlayout", id)
                .with_input(sphere)
                .with_f32("padding", 0.0)
                .with_u32("tiles", 0)),
            field
        );
        // Inputs must already be defined: forward references are ordering errors.
        assert_eq!(
            bad(Node::new("voronoifracture", id)
                .with_input(sphere)
                .with_input(NodeId(9))),
            Err(SceneError::NodeOrder)
        );
        assert_eq!(
            bad(Node::new("attribtransfer", id)
                .with_input(NodeId(9))
                .with_input(sphere)
                .with_text("name", "a".to_owned())
                .with_f32("radius", 1.0)),
            Err(SceneError::NodeOrder)
        );
    }

    #[test]
    fn asset_nodes_round_trip_and_reject_anything_but_a_canonical_hash() {
        let hash = "0123456789abcdef".repeat(4);
        let scene = |content_hash: String| GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::new("asset", NodeId(1)).with_text("content_hash", content_hash),
                Node::new("reverse", NodeId(2)).with_input(NodeId(1)),
            ],
        };
        let good = scene(hash.clone());
        assert_eq!(GraphSnapshot::from_ofxs(&good.to_ofxs()), Ok(good.clone()));
        assert_eq!(good.nodes[0].inputs(), Vec::new());
        for bad in [
            String::new(),
            "abc".to_owned(),
            hash.to_uppercase(),
            format!("{hash}0"),
            format!("../{}", &hash[3..]),
            "g".repeat(64),
        ] {
            assert_eq!(
                scene(bad.clone()).validate(),
                Err(SceneError::Field),
                "{bad}"
            );
        }
    }

    #[test]
    fn malformed_sop_lines_are_rejected_by_the_parser() {
        for body in [
            "sphere 1 1e0 8\n",
            "sphere 1 1e0 8 8 extra\n",
            "line 1 0e0 0e0 0e0 1e0 1e0 5\n",
            "sweep 2 1 1e-1 6 maybe\n",
            "group_bounds 2 1 g vertex 0e0 0e0 0e0 1e0 1e0 1e0\n",
            "attribtransfer 2 1 1 bad-name 1e0\n",
            "polyreduce 2 1 nan\n",
            "voronoifracture 2 1\n",
        ] {
            let text = format!("ofxs 1\nrevision 1\nsphere 1 1e0 8 8\n{body}");
            assert!(GraphSnapshot::from_ofxs(&text).is_err(), "{body}");
        }
        // A consumer cannot be the first line of a scene.
        assert_eq!(
            GraphSnapshot::from_ofxs("ofxs 1\nrevision 1\nreverse 2 1\nsphere 1 1e0 8 8\n"),
            Err(SceneError::NodeOrder)
        );
    }

    #[test]
    fn diamond_dag_apply_edits_and_catalog_introspection() {
        let base = GraphSnapshot::demo(8, Vec3::checked(1.0, 0.0, 0.0).unwrap_or(Vec3::ZERO));
        let edits = vec![
            GraphEdit::AddNode {
                node: Node::new("transform", NodeId(3))
                    .with_input(NodeId(1))
                    .with_vec3("offset", Vec3::checked(0.0, 2.0, 0.0).unwrap_or(Vec3::ZERO)),
            },
            GraphEdit::AddNode {
                node: Node::new("merge", NodeId(4))
                    .with_input(NodeId(2))
                    .with_input(NodeId(3)),
            },
        ];
        let edited = base.apply_edits(RevisionId(2), &edits);
        assert!(edited.is_ok());
        if let Ok(diamond) = edited {
            let ofxs = diamond.to_ofxs();
            assert_eq!(GraphSnapshot::from_ofxs(&ofxs), Ok(diamond.clone()));

            // Introducing a cycle (rewiring node 2 to depend on downstream merge node 4) must be rejected.
            let cycle = diamond.apply_edits(
                RevisionId(3),
                &[GraphEdit::SetInput {
                    id: NodeId(2),
                    slot: 0,
                    input: NodeId(4),
                }],
            );
            assert_eq!(cycle, Err(SceneError::NodeOrder));
        }

        assert_eq!(search_node_catalog("").len(), 47);
        let matches: Vec<String> = search_node_catalog("merge")
            .into_iter()
            .map(|spec| spec.type_name)
            .collect();
        assert_eq!(
            matches,
            ["foreach", "merge"],
            "matched by name or by description"
        );
        assert!(inspect_node_type("transform").is_some());
        assert!(inspect_node_type("grid").is_some());
        assert!(inspect_node_type("box").is_some());
        assert!(inspect_node_type("normal").is_some());
        assert!(inspect_node_type("wrangle").is_some());
        assert!(inspect_node_type("subdivide").is_some());
        assert!(inspect_node_type("extrude").is_some());
        assert!(inspect_node_type("attribpromote").is_some());
        assert!(inspect_node_type("blast").is_some());
        assert!(inspect_node_type("uvproject").is_some());
        assert!(inspect_node_type("heightfield").is_some());
        assert!(inspect_node_type("hf_noise").is_some());
        assert!(inspect_node_type("hf_erode").is_some());
        assert!(inspect_node_type("vdb_from_polygons").is_some());
        assert!(inspect_node_type("vdb_reshape").is_some());
        assert!(inspect_node_type("convert_vdb").is_some());
        assert!(inspect_node_type("boolean").is_some());
        assert!(inspect_node_type("scatter").is_some());
        assert!(inspect_node_type("copytopoints").is_some());
        assert!(inspect_node_type("nonexistent").is_none());
        for spec in node_catalog() {
            assert!(
                !spec.summary.is_empty() && !spec.outputs.is_empty(),
                "{}",
                spec.type_name
            );
        }

        let grid_scatter = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::new("heightfield", NodeId(1))
                    .with_u32("rows", 8)
                    .with_u32("cols", 8)
                    .with_f32("size_x", 10.0)
                    .with_f32("size_z", 20.0),
                Node::new("box", NodeId(2))
                    .with_vec3("size", Vec3::checked(1.0, 2.0, 3.0).unwrap_or(Vec3::ZERO)),
                Node::new("subdivide", NodeId(3))
                    .with_input(NodeId(2))
                    .with_u32("iterations", 2),
                Node::new("extrude", NodeId(4))
                    .with_input(NodeId(3))
                    .with_f32("distance", 0.5)
                    .with_f32("inset", 0.1),
                Node::new("vdb_from_polygons", NodeId(15))
                    .with_input(NodeId(4))
                    .with_f32("voxel_size", 0.5),
                Node::new("vdb_reshape", NodeId(16))
                    .with_input(NodeId(15))
                    .with_f32("offset", 0.1)
                    .with_u32("smooth_iterations", 1),
                Node::new("convert_vdb", NodeId(17)).with_input(NodeId(16)),
                Node::new("boolean", NodeId(18))
                    .with_input(NodeId(2))
                    .with_input(NodeId(17))
                    .with_text("mode", BooleanMode::Union.as_str())
                    .with_f32("voxel_size", 0.5),
                Node::new("normal", NodeId(5)).with_input(NodeId(18)),
                Node::new("uvproject", NodeId(12))
                    .with_input(NodeId(5))
                    .with_text("mode", UvProjectMode::Box.as_str()),
                Node::new("hf_noise", NodeId(13))
                    .with_input(NodeId(1))
                    .with_f32("amplitude", 4.0)
                    .with_f32("frequency", 0.1)
                    .with_u32("octaves", 3)
                    .with_u64("seed", 9),
                Node::new("hf_erode", NodeId(14))
                    .with_input(NodeId(13))
                    .with_u32("iterations", 5)
                    .with_f32("rain_rate", 0.05)
                    .with_f32("talus_angle", 35.0)
                    .with_u64("seed", 11),
                Node::new("scatter", NodeId(6))
                    .with_input(NodeId(14))
                    .with_u32("count", 50)
                    .with_u64("seed", 42),
                Node::new("wrangle", NodeId(7))
                    .with_input(NodeId(6))
                    .with_text(
                        "snippet",
                        "f@pscale = 0.5;\nf@keep = 1.0;\n@P.y = sin(@P.x);".to_owned(),
                    ),
                Node::new("attribpromote", NodeId(8))
                    .with_input(NodeId(7))
                    .with_text("name", "pscale".to_owned())
                    .with_text("from_domain", AttrDomain::Point.as_str())
                    .with_text("to_domain", AttrDomain::Detail.as_str())
                    .with_text("mode", PromoteMode::Average.as_str()),
                Node::new("attribpromote", NodeId(9))
                    .with_input(NodeId(8))
                    .with_text("name", "pscale".to_owned())
                    .with_text("from_domain", AttrDomain::Detail.as_str())
                    .with_text("to_domain", AttrDomain::Point.as_str())
                    .with_text("mode", PromoteMode::Max.as_str()),
                Node::new("blast", NodeId(10))
                    .with_input(NodeId(9))
                    .with_text("group_name", "keep".to_owned())
                    .with_bool("negate", true),
                Node::new("copytopoints", NodeId(11))
                    .with_input(NodeId(12))
                    .with_input(NodeId(10)),
            ],
        };
        let gs_ofxs = grid_scatter.to_ofxs();
        assert_eq!(GraphSnapshot::from_ofxs(&gs_ofxs), Ok(grid_scatter));

        // Malformed input rejection for new node types
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nvdb_from_polygons 2 1 1e-3\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nvdb_from_polygons 2 1 5e-1\nvdb_reshape 3 2 1e-1 999\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nbox 2 1e0 1e0 1e0\nboolean 3 1 2 bad_op 5e-1\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nuvproject 2 1 invalid_mode\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nheightfield 1 1 8 10e0 10e0\nnormal 2 1\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nheightfield 1 8 8 10e0 10e0\nhf_noise 2 1 5e0 1e-1 0 1\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nheightfield 1 8 8 10e0 10e0\nhf_erode 2 1 10 5e-2 95e0 1\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs("ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nsubdivide 2 1 0\n"),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs("ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nsubdivide 2 1 99\n"),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nextrude 2 1 1e0 -1e-1\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nextrude 2 1 NaN 0e0\n"
            ),
            Err(SceneError::NonFinite)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nattribpromote 2 1 P point prim average\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nattribpromote 2 1 heat point point average\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nblast 2 1 1bad false\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nblast 2 1 grp maybe\n"
            ),
            Err(SceneError::Field)
        );
    }
}
