//! Node definitions, the `.ofxs` format they drive, and runtime registration.

use pcc_graph::{
    Check, GraphEdit, GraphSnapshot, Node, NodeDef, NodeInput, NodeParam, NodeRegistrationError,
    ParamKind, ParamValue, SceneError, inspect_node_type, node_catalog, node_def,
    register_node_def, search_node_catalog,
};
use pcc_id::{NodeId, RevisionId};
use pcc_math::Vec3;
use std::collections::BTreeSet;

const GOLDEN: &str = include_str!("golden_all_operators.ofxs");

fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

/// A valid scene ending in `node`, so that scene-level rules do not mask per-node ones.
fn scene_with(node: Node) -> GraphSnapshot {
    GraphSnapshot {
        revision: RevisionId(1),
        nodes: vec![
            Node::new("create", NodeId(1)).with_u32("count", 3),
            node,
            Node::new("reverse", NodeId(99)).with_input(NodeId(1)),
        ],
    }
}

fn validate(node: Node) -> Result<(), SceneError> {
    scene_with(node).validate()
}

#[test]
fn the_format_of_every_operator_is_frozen() {
    let parsed = GraphSnapshot::from_ofxs(GOLDEN);
    assert_eq!(parsed.as_ref().err(), None);
    if let Ok(parsed) = parsed {
        assert_eq!(
            parsed.to_ofxs(),
            GOLDEN,
            "saved scenes must keep loading unchanged"
        );
        let used: BTreeSet<&str> = parsed.nodes.iter().map(|n| n.op.as_str()).collect();
        // `asset` needs a real hash and has its own tests; everything else must be covered, so a
        // new operator cannot quietly skip this check.
        let missing: Vec<String> = node_catalog()
            .into_iter()
            .map(|spec| spec.type_name)
            // Namespaced operators are registered by other tests in this process.
            .filter(|name| !name.contains(':'))
            .filter(|name| name != "asset" && !used.contains(name.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "missing from the golden scene: {missing:?}"
        );
    }
}

#[test]
fn nodes_are_plain_json_and_numbers_settle_to_their_declared_kind() -> Result<(), serde_json::Error>
{
    let json = serde_json::json!({
        "edit": "add_node",
        "node": {
            "id": 5,
            "op": "sphere",
            "params": { "radius": 2, "rows": 8, "cols": 12 }
        }
    });
    let edit: GraphEdit = serde_json::from_value(json)?;
    let base = scene_with(Node::new("reverse", NodeId(2)).with_input(NodeId(1)));
    // A scene may not end on a generator, so something consumes the new sphere.
    let consumer = GraphEdit::AddNode {
        node: Node::new("normal", NodeId(6)).with_input(NodeId(5)),
    };
    let edited = base.apply_edits(RevisionId(2), &[edit, consumer]);
    assert_eq!(edited.as_ref().err(), None);
    if let Ok(edited) = edited {
        let sphere = edited.nodes.iter().find(|n| n.id == NodeId(5));
        // `"radius": 2` arrived as an integer. Stored as a float, it equals its own round trip.
        assert_eq!(
            sphere.and_then(|n| n.params.get("radius")),
            Some(&ParamValue::Float(2.0))
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(&edited.to_ofxs()).as_ref(),
            Ok(&edited)
        );
    }
    // An integer is accepted for a float, but not the other way round.
    assert_eq!(
        validate(Node::new("box", NodeId(2)).with_vec3("size", v3(1.0, 1.0, 1.0))),
        Ok(())
    );
    let fractional: Result<Node, _> = serde_json::from_value(serde_json::json!({
        "id": 2, "op": "sphere", "params": { "radius": 1.0, "rows": 8.5, "cols": 8 }
    }));
    if let Ok(node) = fractional {
        assert_eq!(validate(node), Err(SceneError::Field));
    }
    Ok(())
}

#[test]
fn generic_edits_set_parameters_and_inputs_and_reject_what_does_not_exist() {
    let base = GraphSnapshot {
        revision: RevisionId(1),
        nodes: vec![
            Node::new("create", NodeId(1)).with_u32("count", 3),
            Node::new("create", NodeId(2)).with_u32("count", 4),
            Node::new("merge", NodeId(3))
                .with_input(NodeId(1))
                .with_input(NodeId(1)),
        ],
    };
    let set_param = |id: u64, name: &str, value: ParamValue| GraphEdit::SetParam {
        id: NodeId(id),
        name: name.to_owned(),
        value,
    };
    let edited = base.apply_edits(RevisionId(2), &[set_param(1, "count", ParamValue::Uint(9))]);
    assert_eq!(edited.map(|g| g.nodes[0].u32("count")), Ok(Some(9)));
    let rewired = base.apply_edits(
        RevisionId(2),
        &[GraphEdit::SetInput {
            id: NodeId(3),
            slot: 1,
            input: NodeId(2),
        }],
    );
    assert_eq!(
        rewired.map(|g| g.nodes[2].inputs.clone()),
        Ok(vec![NodeId(1), NodeId(2)])
    );

    let field = Err(SceneError::Field);
    for bad in [
        set_param(1, "no_such_param", ParamValue::Uint(1)),
        set_param(7, "count", ParamValue::Uint(1)),
        set_param(1, "count", ParamValue::Text("many".to_owned())),
        GraphEdit::SetInput {
            id: NodeId(3),
            slot: 2,
            input: NodeId(1),
        },
        GraphEdit::SetInput {
            id: NodeId(7),
            slot: 0,
            input: NodeId(1),
        },
    ] {
        assert_eq!(
            base.apply_edits(RevisionId(2), std::slice::from_ref(&bad)),
            field,
            "{bad:?}"
        );
    }
    // A cycle introduced by rewiring is caught.
    let cycle = base.apply_edits(
        RevisionId(2),
        &[GraphEdit::SetInput {
            id: NodeId(3),
            slot: 0,
            input: NodeId(3),
        }],
    );
    assert_eq!(cycle, Err(SceneError::NodeOrder));
}

#[test]
fn a_node_must_match_its_definition_exactly() {
    let id = NodeId(2);
    let field = Err(SceneError::Field);
    assert_eq!(validate(Node::new("no_such_op", id)), field);
    // Missing parameter, extra parameter, wrong kind.
    assert_eq!(validate(Node::new("box", id)), field);
    let extra = Node::new("box", id)
        .with_vec3("size", v3(1.0, 1.0, 1.0))
        .with_u32("bonus", 1);
    assert_eq!(validate(extra), field);
    assert_eq!(validate(Node::new("box", id).with_u32("size", 1)), field);
    // Too few and too many inputs.
    assert_eq!(validate(Node::new("reverse", id)), field);
    let doubled = Node::new("reverse", id)
        .with_input(NodeId(1))
        .with_input(NodeId(1));
    assert_eq!(validate(doubled), field);
    // An input that comes later in the scene is an ordering error.
    assert_eq!(
        validate(Node::new("reverse", id).with_input(NodeId(50))),
        Err(SceneError::NodeOrder)
    );
}

#[test]
fn every_kind_of_constraint_holds_at_its_boundary() {
    let id = NodeId(2);
    let field = Err(SceneError::Field);
    let reduce = |ratio| {
        Node::new("polyreduce", id)
            .with_input(NodeId(1))
            .with_f32("target_ratio", ratio)
    };
    // Open below, closed above.
    assert_eq!(validate(reduce(0.0)), field);
    assert_eq!(validate(reduce(1e-6)), Ok(()));
    assert_eq!(validate(reduce(1.0)), Ok(()));
    assert_eq!(validate(reduce(1.001)), field);
    assert_eq!(validate(reduce(f32::NAN)), Err(SceneError::NonFinite));
    // Closed below, open above.
    let layout = |padding| {
        Node::new("uvlayout", id)
            .with_input(NodeId(1))
            .with_f32("padding", padding)
            .with_u32("tiles", 1)
    };
    assert_eq!(validate(layout(0.0)), Ok(()));
    assert_eq!(validate(layout(-0.001)), field);
    assert_eq!(validate(layout(0.25)), field);
    // Closed at both ends.
    let bevel = |angle| {
        Node::new("polybevel", id)
            .with_input(NodeId(1))
            .with_f32("distance", 0.1)
            .with_f32("min_angle_degrees", angle)
    };
    assert_eq!(validate(bevel(0.0)), Ok(()));
    assert_eq!(validate(bevel(180.0)), Ok(()));
    assert_eq!(validate(bevel(180.5)), field);
    // A floor.
    let fuse = |distance| {
        Node::new("fuse", id)
            .with_input(NodeId(1))
            .with_f32("distance", distance)
    };
    assert_eq!(validate(fuse(0.0)), Ok(()));
    assert_eq!(validate(fuse(-0.1)), field);
    // Unsigned bounds.
    let subdivide = |n| {
        Node::new("subdivide", id)
            .with_input(NodeId(1))
            .with_u32("iterations", n)
    };
    assert_eq!(validate(subdivide(0)), field);
    assert_eq!(validate(subdivide(1)), Ok(()));
    assert_eq!(validate(subdivide(6)), Ok(()));
    assert_eq!(validate(subdivide(7)), field);
    let line = |points| {
        Node::new("line", id)
            .with_vec3("start", Vec3::ZERO)
            .with_vec3("end", Vec3::ZERO)
            .with_u32("points", points)
    };
    assert_eq!(validate(line(1)), field);
    assert_eq!(validate(line(2)), Ok(()));
    // u64 values beyond u32 are rejected where a u32 is declared.
    let huge = Node::new("create", id).with_u64("count", u64::from(u32::MAX) + 1);
    assert_eq!(validate(huge), field);
    // Vectors.
    let sized = |size| Node::new("box", id).with_vec3("size", size);
    assert_eq!(validate(sized(v3(1.0, 0.0, 1.0))), field);
    assert_eq!(
        validate(sized(v3(1.0, f32::INFINITY, 1.0))),
        Err(SceneError::NonFinite)
    );
    let scaled = |scale| {
        Node::new("xform", id)
            .with_input(NodeId(1))
            .with_vec3("translate", Vec3::ZERO)
            .with_vec3("rotate_degrees", Vec3::ZERO)
            .with_vec3("scale", scale)
    };
    assert_eq!(validate(scaled(v3(-1.0, 1.0, 1.0))), Ok(()));
    assert_eq!(validate(scaled(v3(1.0, 0.0, 1.0))), field);
    let clipped = |normal| {
        Node::new("clip", id)
            .with_input(NodeId(1))
            .with_vec3("origin", Vec3::ZERO)
            .with_vec3("normal", normal)
            .with_bool("cap", true)
    };
    assert_eq!(validate(clipped(v3(0.0, 0.0, 1.0))), Ok(()));
    assert_eq!(validate(clipped(Vec3::ZERO)), field);
    // Text.
    let blast = |name: &str| {
        Node::new("blast", id)
            .with_input(NodeId(1))
            .with_text("group_name", name)
            .with_bool("negate", false)
    };
    assert_eq!(validate(blast("group_1")), Ok(()));
    for bad in ["", "P", "1st", "has space", "a-b"] {
        assert_eq!(validate(blast(bad)), field, "{bad:?}");
    }
    let wrangle = |snippet: &str| {
        Node::new("wrangle", id)
            .with_input(NodeId(1))
            .with_text("snippet", snippet)
    };
    assert_eq!(validate(wrangle("@a = 1;")), Ok(()));
    assert_eq!(validate(wrangle("   ")), field);
    assert_eq!(validate(wrangle("@é = 1;")), field);
    let promote = |mode: &str| {
        Node::new("uvproject", id)
            .with_input(NodeId(1))
            .with_text("mode", mode)
    };
    assert_eq!(validate(promote("box")), Ok(()));
    assert_eq!(validate(promote("BOX")), field);
}

#[test]
fn rules_spanning_several_parameters_are_enforced() {
    let id = NodeId(2);
    let field = Err(SceneError::Field);
    let torus = |major, minor| {
        Node::new("torus", id)
            .with_f32("major_radius", major)
            .with_f32("minor_radius", minor)
            .with_u32("rows", 8)
            .with_u32("cols", 8)
    };
    assert_eq!(validate(torus(2.0, 1.0)), Ok(()));
    assert_eq!(validate(torus(1.0, 1.0)), field);
    let promote = |from: &str, to: &str| {
        Node::new("attribpromote", id)
            .with_input(NodeId(1))
            .with_text("name", "a")
            .with_text("from_domain", from)
            .with_text("to_domain", to)
            .with_text("mode", "sum")
    };
    assert_eq!(validate(promote("point", "prim")), Ok(()));
    assert_eq!(validate(promote("prim", "prim")), field);
    let group = |domain: &str, low, high| {
        Node::new("group_bounds", id)
            .with_input(NodeId(1))
            .with_text("group_name", "g")
            .with_text("domain", domain)
            .with_vec3("min", low)
            .with_vec3("max", high)
    };
    let (zero, one) = (Vec3::ZERO, v3(1.0, 1.0, 1.0));
    assert_eq!(validate(group("prim", zero, one)), Ok(()));
    assert_eq!(validate(group("detail", zero, one)), field);
    assert_eq!(validate(group("prim", one, zero)), field);
}

static WAVE_INPUTS: [NodeInput; 1] = [NodeInput {
    name: "input",
    description: "Geometry to deform.",
}];
static WAVE_PARAMS: [NodeParam; 2] = [
    NodeParam {
        name: "height",
        kind: ParamKind::F32,
        check: Check::Positive,
        description: "Wave height.",
    },
    NodeParam {
        name: "style",
        kind: ParamKind::Choice(&["sine", "square"]),
        check: Check::Any,
        description: "Wave shape.",
    },
];
const WAVE: NodeDef = NodeDef::new(
    "acme:wave",
    "deform",
    "A plugin operator with a unique search word: zigzagoon.",
    &WAVE_INPUTS,
    &WAVE_PARAMS,
);

#[test]
fn a_registered_operator_joins_the_catalog_validation_and_format() {
    assert_eq!(register_node_def(WAVE), Ok(()));
    assert_eq!(
        node_def("acme:wave").map(|def| def.type_name),
        Some("acme:wave")
    );
    assert_eq!(search_node_catalog("zigzagoon").len(), 1);
    let spec = inspect_node_type("acme:wave");
    assert_eq!(spec.as_ref().map(|s| s.params.len()), Some(2));
    assert_eq!(
        spec.map(|s| s.params[1].param_type.clone()),
        Some("string".to_owned())
    );

    let wave = |height| {
        Node::new("acme:wave", NodeId(2))
            .with_input(NodeId(1))
            .with_f32("height", height)
            .with_text("style", "sine")
    };
    assert_eq!(validate(wave(1.0)), Ok(()));
    assert_eq!(validate(wave(0.0)), Err(SceneError::Field));
    let scene = scene_with(wave(1.5));
    let text = scene.to_ofxs();
    assert!(text.contains("acme:wave 2 1 1.5e0 sine\n"), "{text}");
    assert_eq!(GraphSnapshot::from_ofxs(&text).as_ref(), Ok(&scene));
}

#[test]
fn registration_protects_built_ins_and_refuses_malformed_definitions() {
    let named = |name: &'static str| NodeDef::new(name, "test", "A test operator.", &[], &[]);
    assert_eq!(
        register_node_def(named("sphere")),
        Err(NodeRegistrationError::AlreadyDefined)
    );
    assert_eq!(register_node_def(named("test:once")), Ok(()));
    assert_eq!(
        register_node_def(named("test:once")),
        Err(NodeRegistrationError::AlreadyDefined)
    );
    for bad in ["", "Upper", "has space", "1st", "new\nline"] {
        assert_eq!(
            register_node_def(named(bad)),
            Err(NodeRegistrationError::InvalidName),
            "{bad:?}"
        );
    }
    static TWICE: [NodeParam; 2] = [
        NodeParam {
            name: "same",
            kind: ParamKind::U32,
            check: Check::Any,
            description: "First.",
        },
        NodeParam {
            name: "same",
            kind: ParamKind::U32,
            check: Check::Any,
            description: "Second.",
        },
    ];
    let duplicate = NodeDef::new("test:twice", "test", "Repeats a name.", &[], &TWICE);
    assert_eq!(
        register_node_def(duplicate),
        Err(NodeRegistrationError::DuplicateParam)
    );
    assert!(node_def("test:twice").is_none());
}

#[test]
fn every_built_in_definition_is_internally_consistent() {
    for spec in node_catalog() {
        let name = &spec.type_name;
        assert!(!spec.summary.is_empty(), "{name}");
        let params: BTreeSet<&String> = spec.params.iter().map(|p| &p.name).collect();
        assert_eq!(
            params.len(),
            spec.params.len(),
            "{name} repeats a parameter"
        );
        assert!(
            spec.params.iter().all(|p| !p.description.is_empty()),
            "{name}"
        );
        assert!(
            spec.inputs.iter().all(|p| !p.description.is_empty()),
            "{name}"
        );
    }
}

#[test]
fn expressions_round_trip_through_the_format_and_json() -> Result<(), serde_json::Error> {
    let scene = GraphSnapshot {
        revision: RevisionId(1),
        nodes: vec![
            Node::new("sphere", NodeId(1))
                .with_expr("radius", "1 + $F * 0.5")
                .with_expr("rows", "8")
                .with_u32("cols", 12),
            Node::new("xform", NodeId(2))
                .with_input(NodeId(1))
                .with_vec3_expr("translate", ["0", "ch(1, \"radius\")", "sin($T)"])
                .with_vec3("rotate_degrees", Vec3::ZERO)
                .with_vec3("scale", v3(1.0, 1.0, 1.0)),
        ],
    };
    assert_eq!(scene.validate(), Ok(()));
    let text = scene.to_ofxs();
    // Hex keeps spaces and quotes from breaking the line's positional tokens.
    assert!(
        text.lines()
            .all(|line| !line.contains('$') && !line.contains('"')),
        "{text}"
    );
    assert!(
        text.contains("xform 2 1 0e0 ="),
        "literal components stay readable: {text}"
    );
    let parsed = GraphSnapshot::from_ofxs(&text);
    assert_eq!(parsed.as_ref(), Ok(&scene));
    assert_eq!(parsed.map(|p| p.to_ofxs()), Ok(text.clone()));

    // Agents write formulas in plain JSON, and equal scenes compare equal however they arrived.
    let node: Node = serde_json::from_value(serde_json::json!({
        "id": 2,
        "op": "xform",
        "inputs": [1],
        "params": {
            "translate": { "x": "0.0", "y": "ch(1, \"radius\")", "z": "sin($T)" },
            "rotate_degrees": { "x": 0, "y": 0, "z": 0 },
            "scale": { "x": 1, "y": 1, "z": 1 }
        }
    }))?;
    let edited = GraphSnapshot {
        revision: RevisionId(1),
        nodes: vec![scene.nodes[0].clone()],
    }
    .apply_edits(RevisionId(2), &[GraphEdit::AddNode { node }]);
    assert_eq!(
        edited.map(|g| g.to_ofxs().lines().last().map(str::to_owned)),
        Ok(text.lines().last().map(str::to_owned))
    );

    // A vector whose components are all plain numbers settles to an ordinary vector.
    let mut plain = Node::new("box", NodeId(2)).with_vec3_expr("size", ["1", "2.0", "3e0"]);
    plain.canonicalize_numbers();
    assert_eq!(plain.vec3("size"), Some(v3(1.0, 2.0, 3.0)));
    Ok(())
}

#[test]
fn expressions_are_checked_where_they_can_be() {
    let id = NodeId(2);
    let field = Err(SceneError::Field);
    let sphere = |radius: &str| {
        Node::new("sphere", id)
            .with_expr("radius", radius)
            .with_u32("rows", 8)
            .with_u32("cols", 8)
    };
    assert_eq!(validate(sphere("$F + 1")), Ok(()));
    // Syntax fails at once, in time for an agent to see it when the edit is made.
    assert_eq!(validate(sphere("$F +")), field);
    assert_eq!(validate(sphere("nosuch(1)")), field);
    // A range can only fail once there is a value, so a formula may look out of range here.
    assert_eq!(validate(sphere("-1")), Ok(()));
    // Text and choices take no formulas, and vectors need all three components.
    let blast = Node::new("blast", id)
        .with_input(NodeId(1))
        .with_expr("group_name", "1")
        .with_bool("negate", false);
    assert_eq!(validate(blast), field);
    let mode = Node::new("uvproject", id)
        .with_input(NodeId(1))
        .with_expr("mode", "1");
    assert_eq!(validate(mode), field);
    let box_scalar = Node::new("box", id).with_expr("size", "1");
    assert_eq!(validate(box_scalar), field);
    let scalar_vec = Node::new("sphere", id)
        .with_vec3_expr("radius", ["1", "1", "$F"])
        .with_u32("rows", 8)
        .with_u32("cols", 8);
    assert_eq!(validate(scalar_vec), field);
    assert_eq!(
        validate(Node::new("box", id).with_vec3_expr("size", ["1", "$F +", "1"])),
        field
    );
    // Rules wait for values, but apply to literal-only nodes as ever.
    let torus = Node::new("torus", id)
        .with_expr("major_radius", "$F")
        .with_f32("minor_radius", 1.0)
        .with_u32("rows", 8)
        .with_u32("cols", 8);
    assert_eq!(validate(torus), Ok(()));
    // Bools take formulas too.
    let swept = Node::new("sweep", id)
        .with_input(NodeId(1))
        .with_f32("radius", 1.0)
        .with_u32("sides", 8)
        .with_expr("capped", "$F > 3");
    assert_eq!(validate(swept), Ok(()));
    // Malformed hex in a scene file.
    for body in [
        "sphere 2 =zz 8 8\n",
        "sphere 2 =6 8 8\n",
        "sphere 2 = 8 8\n",
        "box 2 =31 1e0\n",
    ] {
        let text = format!("ofxs 1\nrevision 1\ncreate 1 3\n{body}reverse 3 2\n");
        assert_eq!(
            GraphSnapshot::from_ofxs(&text).err(),
            Some(SceneError::Field),
            "{body}"
        );
    }
}

#[test]
fn channel_references_are_dependencies_and_cannot_form_cycles() {
    let reader = |id: u64, source: u64| {
        Node::new("sphere", NodeId(id))
            .with_expr("radius", format!("ch({source}, \"radius\")"))
            .with_u32("rows", 8)
            .with_u32("cols", 8)
    };
    let base = Node::new("sphere", NodeId(1))
        .with_f32("radius", 1.0)
        .with_u32("rows", 8)
        .with_u32("cols", 8);
    let end = Node::new("reverse", NodeId(9)).with_input(NodeId(1));
    assert_eq!(reader(5, 3).dependencies(), vec![NodeId(3)]);
    // Reading an earlier node is fine, including alongside real inputs.
    let ok = GraphSnapshot {
        revision: RevisionId(1),
        nodes: vec![base.clone(), reader(2, 1), end.clone()],
    };
    assert_eq!(ok.validate(), Ok(()));
    // Reading a later node, itself, or nothing that exists is an ordering error.
    for bad in [
        vec![base.clone(), reader(2, 3), reader(3, 1), end.clone()],
        vec![base.clone(), reader(2, 2), end.clone()],
        vec![base.clone(), reader(2, 77), end.clone()],
    ] {
        let scene = GraphSnapshot {
            revision: RevisionId(1),
            nodes: bad,
        };
        assert_eq!(scene.validate(), Err(SceneError::NodeOrder));
    }
    // Edits sort by dependencies, so a read-before-definition order is repaired...
    let start = GraphSnapshot {
        revision: RevisionId(1),
        nodes: vec![base.clone(), end.clone()],
    };
    let added = start.apply_edits(
        RevisionId(2),
        &[
            GraphEdit::AddNode { node: reader(4, 3) },
            GraphEdit::AddNode { node: reader(3, 1) },
            GraphEdit::AddNode {
                node: Node::new("reverse", NodeId(8)).with_input(NodeId(4)),
            },
        ],
    );
    let order = added.map(|g| g.nodes.iter().map(|n| n.id.0).collect::<Vec<_>>());
    let order = order.unwrap_or_default();
    let at = |id: u64| order.iter().position(|&n| n == id);
    assert!(at(3) < at(4), "{order:?}");
    // ...and a cycle through expressions alone is rejected.
    let cycle = start.apply_edits(
        RevisionId(2),
        &[
            GraphEdit::AddNode { node: reader(4, 3) },
            GraphEdit::AddNode { node: reader(3, 4) },
        ],
    );
    assert_eq!(cycle, Err(SceneError::NodeOrder));
}

fn zone_box(id: u64) -> Node {
    Node::new("box", NodeId(id)).with_vec3("size", v3(1.0, 1.0, 1.0))
}

fn prev(id: u64, zone: u64) -> Node {
    Node::new("state_prev", NodeId(id)).with_u64("zone", zone)
}

fn step(id: u64, input: u64) -> Node {
    Node::new("reverse", NodeId(id)).with_input(NodeId(input))
}

fn solver(id: u64, initial: u64, output: u64) -> Node {
    Node::new("solver", NodeId(id))
        .with_input(NodeId(initial))
        .with_input(NodeId(output))
        .with_u32("ticks", 3)
}

fn snapshot(nodes: Vec<Node>) -> GraphSnapshot {
    GraphSnapshot {
        revision: RevisionId(1),
        nodes,
    }
}

#[test]
fn a_zone_lists_its_members_in_scene_order() {
    let scene = snapshot(vec![
        zone_box(1),
        zone_box(2),
        prev(3, 7),
        step(4, 3),
        // A node reading outside the body too is still part of it.
        Node::new("merge", NodeId(5))
            .with_input(NodeId(4))
            .with_input(NodeId(2)),
        step(6, 5),
        solver(7, 1, 6),
        step(8, 7),
    ]);
    assert_eq!(scene.validate(), Ok(()));
    let zones = scene.zones();
    assert_eq!(zones.as_ref().map(Vec::len), Ok(1));
    if let Ok(zones) = zones {
        let zone = &zones[0];
        assert_eq!(zone.owner, NodeId(7));
        assert_eq!(zone.source, NodeId(1));
        assert_eq!(zone.output, NodeId(6));
        assert_eq!(zone.entries, vec![NodeId(3)]);
        assert_eq!(zone.body, vec![NodeId(3), NodeId(4), NodeId(5), NodeId(6)]);
    }
    // The body may be nothing but its entry.
    let identity = snapshot(vec![zone_box(1), prev(2, 3), solver(3, 1, 2)]);
    assert_eq!(identity.validate(), Ok(()));
}

#[test]
fn zones_are_sealed_so_no_value_escapes_except_the_committed_result() {
    let order = Err(SceneError::NodeOrder);
    let field = Err(SceneError::Field);
    let check = |nodes: Vec<Node>| snapshot(nodes).validate();

    // An entry must name an existing owner of its own kind.
    assert_eq!(check(vec![zone_box(1), prev(2, 77), step(3, 2)]), order);
    assert_eq!(check(vec![zone_box(1), prev(2, 1), step(3, 2)]), order);
    let wrong_kind = Node::new("foreach_piece", NodeId(2)).with_u64("zone", 3);
    assert_eq!(check(vec![zone_box(1), wrong_kind, solver(3, 1, 2)]), order);

    // Outside code cannot read a live value from inside, by input or by channel.
    let peek = vec![
        zone_box(1),
        prev(2, 4),
        step(3, 2),
        solver(4, 1, 3),
        step(5, 3),
    ];
    assert_eq!(check(peek), order);
    let channel = Node::new("sphere", NodeId(5))
        .with_expr("radius", "ch(3, \"nope\")")
        .with_u32("rows", 4)
        .with_u32("cols", 4);
    let peek = vec![
        zone_box(1),
        prev(2, 4),
        step(3, 2),
        solver(4, 1, 3),
        channel,
        step(6, 5),
    ];
    assert_eq!(check(peek), order);

    // The owner itself may read its output, but nothing else inside.
    let snooping = Node::new("solver", NodeId(5))
        .with_input(NodeId(1))
        .with_input(NodeId(4))
        .with_expr("ticks", "ch(3, \"nope\")");
    let peek = vec![zone_box(1), prev(2, 5), step(3, 2), step(4, 3), snooping];
    assert_eq!(check(peek), order);

    // A body that ignores its state would only repeat a constant.
    let ignores = vec![
        zone_box(1),
        zone_box(2),
        prev(3, 5),
        step(4, 2),
        solver(5, 1, 4),
    ];
    assert_eq!(check(ignores), field);
    // An entry that feeds nothing is a mistake, not a no-op.
    let unused = vec![
        zone_box(1),
        prev(2, 5),
        prev(3, 5),
        step(4, 2),
        solver(5, 1, 4),
    ];
    assert_eq!(check(unused), order);

    // The initial state must exist before the body, and come from outside it.
    let late = vec![zone_box(1), prev(2, 4), zone_box(3), solver(4, 3, 2)];
    assert_eq!(check(late), order);
    let from_inside = vec![zone_box(1), prev(2, 4), step(3, 2), solver(4, 3, 3)];
    assert_eq!(check(from_inside), order);

    // Nested zones are not supported yet, and must say so.
    let nested = vec![
        zone_box(1),
        prev(2, 6),
        prev(3, 4),
        solver(4, 2, 3),
        step(5, 4),
        solver(6, 1, 5),
    ];
    assert_eq!(check(nested), field);
    let shared = vec![
        zone_box(1),
        prev(2, 5),
        prev(3, 6),
        Node::new("merge", NodeId(4))
            .with_input(NodeId(2))
            .with_input(NodeId(3)),
        solver(5, 1, 4),
        solver(6, 1, 4),
    ];
    // Sharing a body breaks the seal, since the second owner reads inside the first zone.
    assert_eq!(check(shared), order);
}

#[test]
fn edits_order_an_entry_after_its_source_and_can_build_a_zone() {
    let base = snapshot(vec![zone_box(1), step(2, 1)]);
    // The entry has no dependency of its own, so only the zone rule can place it after node 10.
    let edited = base.apply_edits(
        RevisionId(2),
        &[
            GraphEdit::AddNode { node: prev(3, 12) },
            GraphEdit::AddNode { node: step(4, 3) },
            GraphEdit::AddNode { node: zone_box(10) },
            GraphEdit::AddNode {
                node: solver(12, 10, 4),
            },
        ],
    );
    assert_eq!(edited.as_ref().err(), None);
    if let Ok(edited) = edited {
        let at = |id: u64| edited.nodes.iter().position(|n| n.id == NodeId(id));
        assert!(at(10) < at(3), "the source must precede the entry");
        assert!(at(4) < at(12));
        assert_eq!(
            GraphSnapshot::from_ofxs(&edited.to_ofxs()).as_ref(),
            Ok(&edited)
        );
    }
}
