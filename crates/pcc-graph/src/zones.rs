//! State zones: the only place a value flows from one evaluation to the next.
//!
//! A zone is built from ordinary nodes, so the graph stays a DAG. An entry node (`state_prev`,
//! `foreach_piece`) names its owner (`solver`, `foreach`) by id rather than through an input. The
//! nodes between the entry and the owner's second input form the body, which the evaluator runs
//! once per tick or piece. Validation makes the body sealed: nothing outside may read a value
//! inside, so a value's only way out is the owner's committed result.

use crate::{GraphSnapshot, Node, SceneError};
use pcc_id::NodeId;
use std::collections::{BTreeMap, BTreeSet};

pub const SOLVER_OP: &str = "solver";
pub const STATE_PREV_OP: &str = "state_prev";
pub const FOREACH_OP: &str = "foreach";
pub const FOREACH_PIECE_OP: &str = "foreach_piece";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneKind {
    /// Runs the body `ticks` times, each from the previous tick's result.
    Solver,
    /// Runs the body once per piece of the input and merges the results.
    ForEach,
}

/// One validated zone, with every member listed in scene order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateZone {
    pub kind: ZoneKind,
    pub owner: NodeId,
    /// The initial state, or the geometry being split into pieces.
    pub source: NodeId,
    /// The body's result.
    pub output: NodeId,
    pub entries: Vec<NodeId>,
    pub body: Vec<NodeId>,
}

fn owner_kind(op: &str) -> Option<ZoneKind> {
    match op {
        SOLVER_OP => Some(ZoneKind::Solver),
        FOREACH_OP => Some(ZoneKind::ForEach),
        _ => None,
    }
}

fn entry_kind(op: &str) -> Option<ZoneKind> {
    match op {
        STATE_PREV_OP => Some(ZoneKind::Solver),
        FOREACH_PIECE_OP => Some(ZoneKind::ForEach),
        _ => None,
    }
}

/// The owner an entry node names.
fn entry_owner(node: &Node) -> Option<NodeId> {
    entry_kind(&node.op)?;
    node.u64("zone").map(NodeId)
}

/// An entry supplies its owner's source to the body, so that source must evaluate first.
/// Returns `(entry, source)` pairs, which are ordering edges beyond the ones nodes declare.
pub(crate) fn entry_source_edges(nodes: &[Node]) -> Vec<(NodeId, NodeId)> {
    let sources: BTreeMap<NodeId, NodeId> = nodes
        .iter()
        .filter(|node| owner_kind(&node.op).is_some())
        .filter_map(|node| Some((node.id, *node.inputs.first()?)))
        .collect();
    nodes
        .iter()
        .filter_map(|node| Some((node.id, *sources.get(&entry_owner(node)?)?)))
        .collect()
}

impl GraphSnapshot {
    /// Finds every zone and checks that it is sealed. Nodes must already be in dependency order.
    pub fn zones(&self) -> Result<Vec<StateZone>, SceneError> {
        let by_id: BTreeMap<NodeId, &Node> = self.nodes.iter().map(|n| (n.id, n)).collect();
        for node in &self.nodes {
            let Some(owner) = entry_owner(node) else {
                continue;
            };
            // An entry and its owner must exist, and be of the same kind.
            let owner_is_valid = by_id
                .get(&owner)
                .is_some_and(|found| owner_kind(&found.op) == entry_kind(&node.op));
            if !owner_is_valid {
                return Err(SceneError::NodeOrder);
            }
        }
        let mut zones = Vec::new();
        for node in &self.nodes {
            let Some(kind) = owner_kind(&node.op) else {
                continue;
            };
            let zone = self.sealed_zone(node, kind)?;
            // Nesting one zone inside another is not supported yet. Overlap is impossible anyway:
            // a second zone reading this body would break the seal checked above.
            let nests = zone
                .body
                .iter()
                .any(|id| by_id.get(id).is_some_and(|n| owner_kind(&n.op).is_some()));
            if nests {
                return Err(SceneError::Field);
            }
            zones.push(zone);
        }
        Ok(zones)
    }

    fn sealed_zone(&self, owner: &Node, kind: ZoneKind) -> Result<StateZone, SceneError> {
        let [source, output] = owner.inputs[..] else {
            return Err(SceneError::Field);
        };
        let entries: BTreeSet<NodeId> = self
            .nodes
            .iter()
            .filter(|node| entry_owner(node) == Some(owner.id))
            .map(|node| node.id)
            .collect();
        // Downstream of an entry, walking forward through dependency order.
        let mut downstream = entries.clone();
        for node in &self.nodes {
            if node.dependencies().iter().any(|d| downstream.contains(d)) {
                downstream.insert(node.id);
            }
        }
        // Upstream of the body's output, walking backward.
        let mut upstream = BTreeSet::from([output]);
        for node in self.nodes.iter().rev() {
            if upstream.contains(&node.id) {
                upstream.extend(node.dependencies());
            }
        }
        // A body must read its state, or the owner would just repeat a constant.
        if !downstream.contains(&output) {
            return Err(SceneError::Field);
        }
        let body: BTreeSet<NodeId> = downstream.intersection(&upstream).copied().collect();
        // Every entry must feed the output. A source inside the body cannot get this far: it would
        // come after the entry that needs it, which the ordering check refuses.
        if !entries.is_subset(&body) {
            return Err(SceneError::NodeOrder);
        }
        for node in &self.nodes {
            let reads_body = node.dependencies().iter().any(|d| body.contains(d));
            if node.id == owner.id {
                // The owner may read the output, but no other body node.
                let reads_other = owner
                    .dependencies()
                    .iter()
                    .any(|d| *d != output && body.contains(d));
                if reads_other {
                    return Err(SceneError::NodeOrder);
                }
            } else if reads_body && !body.contains(&node.id) {
                return Err(SceneError::NodeOrder);
            }
        }
        Ok(StateZone {
            kind,
            owner: owner.id,
            source,
            output,
            entries: entries.into_iter().collect(),
            body: self
                .nodes
                .iter()
                .map(|node| node.id)
                .filter(|id| body.contains(id))
                .collect(),
        })
    }
}
