//! Turns polygon soup into an oriented manifold surface, without moving a single point.
//!
//! Each step removes one kind of defect: faces with repeated corners, repeated faces, inconsistent
//! winding, then edges and points shared by too many faces, and optionally holes. Only connectivity
//! changes, so the shape stays where it was. Self-intersections are geometric and remain.

use crate::common::{
    D3, SopContext, any_perpendicular, attach_prims, cross, dot, length, load_prims, normalized,
    polygon_area_vector, sub,
};
use pcc_geo::{
    AttrDomain, AttributeColumn, GeoDetail, GeoError, GroupDomain, PrimKind, TopologyIndex,
};
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

/// Largest hole that can be filled: the triangulation costs the cube of its edge count.
pub const MAX_FILLED_HOLE_EDGES: usize = 512;

/// Primitive group holding the faces that fill holes.
pub const FILLED_GROUP: &str = "repair_filled";

const NO_POINT: u32 = u32::MAX;
const NO_SIDE: u32 = u32::MAX;

fn is_clean_manifold(geo: &GeoDetail) -> bool {
    let topology = TopologyIndex::build(geo);
    topology.is_manifold() && topology.non_manifold_vertex_count(geo) == 0
}

/// One polygon being repaired, and where its data came from in the input.
struct Face {
    prim: usize,
    points: Vec<u32>,
    vertices: Vec<usize>,
}

/// One face's use of an edge. `lo` and `hi` are the edge's points in ascending order, and `forward`
/// says that the face runs from `lo` to `hi`.
#[derive(Clone, Copy)]
struct Side {
    lo: u32,
    hi: u32,
    forward: bool,
    face: u32,
    corner: u32,
}

#[derive(Default)]
struct Report {
    degenerate: usize,
    duplicate: usize,
    back_to_back: usize,
    flipped: usize,
    split_points: usize,
    filled_holes: usize,
    filled_faces: usize,
    open_holes: usize,
}

impl Report {
    const fn changed_anything(&self) -> bool {
        self.degenerate
            + self.duplicate
            + self.back_to_back
            + self.flipped
            + self.split_points
            + self.filled_holes
            > 0
    }

    fn write(&self, geo: GeoDetail) -> Result<GeoDetail, GeoError> {
        let mut out = geo;
        for (name, count) in [
            ("repair_degenerate_faces", self.degenerate),
            ("repair_duplicate_faces", self.duplicate),
            ("repair_back_to_back_faces", self.back_to_back),
            ("repair_flipped_faces", self.flipped),
            ("repair_split_points", self.split_points),
            ("repair_filled_holes", self.filled_holes),
            ("repair_filled_faces", self.filled_faces),
            ("repair_open_holes", self.open_holes),
        ] {
            let value = i32::try_from(count).map_err(|_| GeoError::LimitExceeded)?;
            out = out.with_domain_attribute(
                AttrDomain::Detail,
                name,
                AttributeColumn::i32_values(vec![value]),
            )?;
        }
        Ok(out)
    }
}

/// Repairs a polygon mesh's connectivity, so that the result passes every manifold test: each edge
/// joins at most two faces, running in opposite directions, and each point's faces form one fan.
///
/// - **Degenerate faces**, whose corners repeat a point, are removed.
/// - **Repeated faces** are reduced to one. When copies face both ways in equal numbers they are all
///   removed, as they are an internal wall between two solids.
/// - **Winding** is made consistent across each connected patch, keeping whichever direction covers
///   more of its area. So an inside-out solid and the inward-facing shell of a cavity stay as they
///   are; use `reverse` to turn a whole mesh.
/// - **Edges with too many faces** are split so that faces pair up around the solid they enclose,
///   and **pinched points** are split into one point per fan. Copies are appended after the
///   existing points, with the attributes and groups of their original.
/// - **Holes** of up to `max_hole_edges` edges are filled with triangles that minimize area, and
///   those faces are placed in the [`FILLED_GROUP`] primitive group. Zero fills nothing. Holes over
///   [`MAX_FILLED_HOLE_EDGES`] stay open, as do any that cannot be filled without a repeated edge.
///
/// Input that needs no repair comes back unchanged. Anything else gains `repair_*` detail attributes
/// counting what was done, including `repair_open_holes` for boundaries left open.
///
/// Near-coincident points are not welded: run `fuse` first. Zero-area faces with three distinct
/// points stay, because removing one would open a hole. Self-intersections stay.
///
/// # Errors
///
/// [`GeoError::InvalidTopology`] when the input holds anything but polygons, or when every face is
/// removed, which means the input enclosed nothing. [`GeoError::Cancelled`] and
/// [`GeoError::LimitExceeded`] as usual.
pub fn mesh_repair(
    geo: &GeoDetail,
    max_hole_edges: usize,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    let mut report = Report::default();
    let loaded = load_faces(geo, &mut report)?;
    if loaded.is_empty() && report.degenerate == 0 {
        return Ok(geo.clone());
    }
    let faces = remove_repeated(loaded, &mut report, ctx)?;
    if faces.is_empty() {
        return Err(GeoError::InvalidTopology);
    }
    let positions = crate::common::load_points(geo)?;
    let sides = build_sides(&faces);
    let groups = edge_groups(&sides);
    let oriented = orient(&faces, &sides, &groups, &positions, ctx)?;
    report.flipped = oriented.flips.iter().filter(|&&flip| flip).count();
    let pairs = pair_sides(&faces, &sides, &groups, &oriented, &positions);
    let flips = oriented.flips;

    let bases = corner_bases(&faces)?;
    let (ids, sources) = assign_points(geo, &faces, &sides, &pairs, &flips, &bases, ctx)?;
    report.split_points = sources.len();

    let mut built = assemble(&faces, &flips, &bases, &ids);
    fill_holes(
        &mut built,
        &positions,
        &sources,
        max_hole_edges,
        &mut report,
        ctx,
    )?;
    if !report.changed_anything() {
        return Ok(geo.clone());
    }
    let out = report.write(build_output(geo, &built, &sources, ctx)?)?;

    if !is_clean_manifold(&out) {
        return Err(GeoError::InvalidTopology);
    }
    Ok(out)
}

/// Reads the polygons, dropping repeated corners. Anything but polygons is refused rather than lost.
fn load_faces(geo: &GeoDetail, report: &mut Report) -> Result<Vec<Face>, GeoError> {
    let mut faces = Vec::with_capacity(geo.prim_count());
    let mut vertex = 0usize;
    for (prim, view) in load_prims(geo)?.into_iter().enumerate() {
        if view.kind != PrimKind::Polygon {
            return Err(GeoError::InvalidTopology);
        }
        let count = view.points.len();
        let mut points = Vec::with_capacity(count);
        let mut vertices = Vec::with_capacity(count);
        for k in 0..count {
            if view.points[k] != view.points[(k + 1) % count] {
                points.push(view.points[k]);
                vertices.push(vertex + k);
            }
        }
        vertex += count;
        let mut sorted = points.clone();
        sorted.sort_unstable();
        sorted.dedup();
        if points.len() < 3 || sorted.len() != points.len() {
            report.degenerate += 1;
            continue;
        }
        faces.push(Face {
            prim,
            points,
            vertices,
        });
    }
    Ok(faces)
}

/// Whether two faces over the same points run the same way round.
fn same_direction(first: &[u32], other: &[u32]) -> bool {
    let count = first.len();
    let Some(start) = other.iter().position(|&point| point == first[0]) else {
        return true;
    };
    let backward = (0..count).all(|k| other[(start + count - k) % count] == first[k]);
    !backward
}

/// Reduces each set of faces over the same points to one, or to none when they cancel.
fn remove_repeated(
    faces: Vec<Face>,
    report: &mut Report,
    ctx: &SopContext<'_>,
) -> Result<Vec<Face>, GeoError> {
    let keys: Vec<Vec<u32>> = faces
        .iter()
        .map(|face| {
            let mut key = face.points.clone();
            key.sort_unstable();
            key
        })
        .collect();
    let mut order: Vec<usize> = (0..faces.len()).collect();
    order.sort_unstable_by(|&a, &b| keys[a].cmp(&keys[b]).then(a.cmp(&b)));
    let mut keep = vec![true; faces.len()];
    for (group_index, group) in order.chunk_by(|&a, &b| keys[a] == keys[b]).enumerate() {
        ctx.poll_every(group_index)?;
        if let [first, rest @ ..] = group
            && !rest.is_empty()
        {
            let same: Vec<bool> = group
                .iter()
                .map(|&face| same_direction(&faces[*first].points, &faces[face].points))
                .collect();
            let forward = same.iter().filter(|&&is_same| is_same).count();
            let backward = group.len() - forward;
            match forward.cmp(&backward) {
                std::cmp::Ordering::Equal => {
                    report.back_to_back += group.len();
                    group.iter().for_each(|&face| keep[face] = false);
                }
                ordering => {
                    report.duplicate += group.len() - 1;
                    let majority = ordering == std::cmp::Ordering::Greater;
                    let survivor = same.iter().position(|&is_same| is_same == majority);
                    for (at, &face) in group.iter().enumerate() {
                        keep[face] = Some(at) == survivor;
                    }
                }
            }
        }
    }
    Ok(faces
        .into_iter()
        .zip(keep)
        .filter_map(|(face, kept)| kept.then_some(face))
        .collect())
}

fn build_sides(faces: &[Face]) -> Vec<Side> {
    let mut sides = Vec::with_capacity(faces.iter().map(|face| face.points.len()).sum());
    for (index, face) in faces.iter().enumerate() {
        let count = face.points.len();
        for k in 0..count {
            let (from, to) = (face.points[k], face.points[(k + 1) % count]);
            let (lo, hi, forward) = match from.cmp(&to) {
                std::cmp::Ordering::Less => (from, to, true),
                _ => (to, from, false),
            };
            sides.push(Side {
                lo,
                hi,
                forward,
                face: index as u32,
                corner: k as u32,
            });
        }
    }
    sides.sort_unstable_by_key(|side| (side.lo, side.hi, side.face, side.corner));
    sides
}

fn edge_groups(sides: &[Side]) -> Vec<Range<usize>> {
    let mut groups = Vec::new();
    let mut offset = 0usize;
    for chunk in sides.chunk_by(|a, b| a.lo == b.lo && a.hi == b.hi) {
        let end = offset + chunk.len();
        groups.push(offset..end);
        offset = end;
    }
    groups
}

/// The winding each face needs, so that each patch, connected across edges with exactly two faces,
/// runs one way.
struct Oriented {
    flips: Vec<bool>,
    /// Each face's patch.
    patches: Vec<u32>,
    /// Edge groups whose two faces cannot agree, as on a Mobius strip.
    conflicts: Vec<bool>,
}

fn orient(
    faces: &[Face],
    sides: &[Side],
    groups: &[Range<usize>],
    positions: &[D3],
    ctx: &SopContext<'_>,
) -> Result<Oriented, GeoError> {
    const UNSEEN: u32 = u32::MAX;
    // (face, neighbour, whether they run the same way, edge group)
    let mut links: Vec<(u32, u32, bool, u32)> = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        if let [a, b] = &sides[group.clone()] {
            let same = a.forward == b.forward;
            links.push((a.face, b.face, same, index as u32));
            links.push((b.face, a.face, same, index as u32));
        }
    }
    links.sort_unstable();
    let mut link_start = vec![0usize; faces.len() + 1];
    for link in &links {
        link_start[link.0 as usize + 1] += 1;
    }
    for face in 0..faces.len() {
        link_start[face + 1] += link_start[face];
    }
    let mut flips = vec![false; faces.len()];
    let mut patch = vec![UNSEEN; faces.len()];
    let mut conflicts = vec![false; groups.len()];
    let mut stack = Vec::new();
    let mut members = Vec::new();
    let mut next_patch = 0u32;
    for start in 0..faces.len() {
        ctx.poll_every(start)?;
        if patch[start] != UNSEEN {
            continue;
        }
        patch[start] = next_patch;
        stack.push(start);
        members.clear();
        while let Some(face) = stack.pop() {
            members.push(face);
            for &(_, neighbour, same, group) in &links[link_start[face]..link_start[face + 1]] {
                let neighbour = neighbour as usize;
                let wanted = flips[face] ^ same;
                if patch[neighbour] == UNSEEN {
                    patch[neighbour] = next_patch;
                    flips[neighbour] = wanted;
                    stack.push(neighbour);
                } else if flips[neighbour] != wanted {
                    conflicts[group as usize] = true;
                }
            }
        }
        // Keep whichever winding already covers more of the patch.
        let (mut kept, mut flipped) = (0.0f64, 0.0f64);
        for &face in &members {
            let area = length(polygon_area_vector(positions, &faces[face].points));
            if flips[face] {
                flipped += area;
            } else {
                kept += area;
            }
        }
        if flipped > kept {
            for &face in &members {
                flips[face] = !flips[face];
            }
        }
        next_patch += 1;
    }
    Ok(Oriented {
        flips,
        patches: patch,
        conflicts,
    })
}

/// Decides which faces join across each edge, as pairs of indices into `sides`.
fn pair_sides(
    faces: &[Face],
    sides: &[Side],
    groups: &[Range<usize>],
    oriented: &Oriented,
    positions: &[D3],
) -> Vec<(u32, u32)> {
    let mut pairs = Vec::with_capacity(groups.len());
    for (index, group) in groups.iter().enumerate() {
        match group.len() {
            1 => {}
            2 => {
                if !oriented.conflicts[index] {
                    pairs.push((group.start as u32, group.start as u32 + 1));
                }
            }
            _ => pair_around_edge(faces, sides, group.clone(), oriented, positions, &mut pairs),
        }
    }
    pairs
}

/// Pairs the faces around one edge so that each pair bounds the solid between them.
///
/// Looking along the edge from `lo` to `hi`, a face running forward has its outside at increasing
/// angles and a face running backward has its inside there. So a solid wedge opens at a backward
/// face and closes at the next forward face, and the pairs match like brackets. Faces left without
/// a partner become boundary.
///
/// On an inside-out solid those brackets enclose the space between solids, and joining across it
/// would fuse them at this edge. The faces cannot tell which they are, but connectivity can: solid
/// wedges tend to pair faces of one patch. So both readings are tried, and the one pairing more
/// faces within a patch wins, with ties going to the outward-facing reading.
fn pair_around_edge(
    faces: &[Face],
    sides: &[Side],
    group: Range<usize>,
    oriented: &Oriented,
    positions: &[D3],
    pairs: &mut Vec<(u32, u32)>,
) {
    let first = sides[group.start];
    let origin = positions[first.lo as usize];
    let axis = normalized(sub(positions[first.hi as usize], origin)).unwrap_or([1.0, 0.0, 0.0]);
    let across = any_perpendicular(axis);
    let up = cross(axis, across);
    let mut around: Vec<(f64, usize)> = group
        .map(|at| {
            let side = sides[at];
            let face = &faces[side.face as usize];
            let beyond = face.points[(side.corner as usize + 2) % face.points.len()];
            let offset = sub(positions[beyond as usize], origin);
            (dot(offset, up).atan2(dot(offset, across)), at)
        })
        .collect();
    around.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

    let order: Vec<usize> = around.iter().map(|entry| entry.1).collect();
    let solid = match_brackets(sides, &order, &oriented.flips, false);
    let inverted = match_brackets(sides, &order, &oriented.flips, true);
    let within_patch = |candidate: &[(u32, u32)]| {
        candidate
            .iter()
            .filter(|&&(a, b)| {
                let patch = |side: u32| oriented.patches[sides[side as usize].face as usize];
                patch(a) == patch(b)
            })
            .count()
    };
    if within_patch(&inverted) > within_patch(&solid) {
        pairs.extend(inverted);
    } else {
        pairs.extend(solid);
    }
}

/// Matches `order`, the sides by increasing angle, as brackets. Backward faces open and forward faces
/// close, or the reverse when `inverted`.
fn match_brackets(
    sides: &[Side],
    order: &[usize],
    flips: &[bool],
    inverted: bool,
) -> Vec<(u32, u32)> {
    let count = order.len();
    let mut pairs = Vec::new();
    let mut taken = vec![false; count];
    let mut open: Vec<usize> = Vec::new();
    let is_closer = |at: usize| {
        let side = sides[order[at]];
        side.forward ^ flips[side.face as usize] ^ inverted
    };
    // First sweep matches brackets in angular order and collects unmatched openers.
    for at in 0..count {
        if is_closer(at) {
            if let Some(opener) = open.pop() {
                taken[opener] = true;
                taken[at] = true;
                pairs.push((order[opener] as u32, order[at] as u32));
            }
        } else {
            open.push(at);
        }
    }
    // Second sweep matches remaining openers with closers that wrapped past the sort seam.
    for at in 0..count {
        if !taken[at]
            && is_closer(at)
            && let Some(opener) = open.pop()
        {
            taken[opener] = true;
            taken[at] = true;
            pairs.push((order[opener] as u32, order[at] as u32));
        }
    }
    pairs
}

fn corner_bases(faces: &[Face]) -> Result<Vec<u32>, GeoError> {
    let mut bases = Vec::with_capacity(faces.len());
    let mut next = 0usize;
    for face in faces {
        bases.push(u32::try_from(next).map_err(|_| GeoError::LimitExceeded)?);
        next += face.points.len();
    }
    u32::try_from(next).map_err(|_| GeoError::LimitExceeded)?;
    Ok(bases)
}

/// The corners of `side` at its `lo` and `hi` points.
fn side_corners(side: Side, faces: &[Face], bases: &[u32]) -> (u32, u32) {
    let base = bases[side.face as usize];
    let count = faces[side.face as usize].points.len() as u32;
    let from = base + side.corner;
    let to = base + (side.corner + 1) % count;
    if side.forward { (from, to) } else { (to, from) }
}

fn find(parents: &mut [u32], at: u32) -> u32 {
    let parent = parents[at as usize];
    if parent == at {
        return at;
    }
    let root = find(parents, parent);
    parents[at as usize] = root;
    root
}

fn union(parents: &mut [u32], a: u32, b: u32) {
    let (a, b) = (find(parents, a), find(parents, b));
    parents[b as usize] = a;
}

/// Gives each corner its output point: corners joined through paired edges share one, and any point
/// with several such fans gets a copy for each fan after the first.
///
/// Pairing alone is not enough. Faces can still share both ends of an edge through fans that go the
/// other way round, as on a Mobius strip or where two pairs meet at one edge. Each such clash cuts a
/// fan open and the assignment is redone, which always ends because every round removes a join.
///
/// Returns each corner's point, and the original of each appended copy.
fn assign_points(
    geo: &GeoDetail,
    faces: &[Face],
    sides: &[Side],
    pairs: &[(u32, u32)],
    flips: &[bool],
    bases: &[u32],
    ctx: &SopContext<'_>,
) -> Result<(Vec<u32>, Vec<u32>), GeoError> {
    let corners: usize = faces.iter().map(|face| face.points.len()).sum();
    let mut corner_point = Vec::with_capacity(corners);
    for face in faces {
        corner_point.extend_from_slice(&face.points);
    }
    let mut partner = vec![NO_SIDE; sides.len()];
    for &(a, b) in pairs {
        partner[a as usize] = b;
        partner[b as usize] = a;
    }
    // The side leaving each corner.
    let mut side_from = vec![NO_SIDE; corners];
    for (index, side) in sides.iter().enumerate() {
        side_from[(bases[side.face as usize] + side.corner) as usize] = index as u32;
    }
    loop {
        ctx.poll()?;
        let mut parents: Vec<u32> = (0..corners as u32).collect();
        for (index, &other) in partner.iter().enumerate() {
            if other != NO_SIDE {
                let (a_lo, a_hi) = side_corners(sides[index], faces, bases);
                let (b_lo, b_hi) = side_corners(sides[other as usize], faces, bases);
                union(&mut parents, a_lo, b_lo);
                union(&mut parents, a_hi, b_hi);
            }
        }
        let mut root_point = vec![NO_POINT; corners];
        let mut point_used = vec![false; geo.len()];
        let mut sources: Vec<u32> = Vec::new();
        let mut ids = vec![NO_POINT; corners];
        for corner in 0..corners as u32 {
            ctx.poll_every(corner as usize)?;
            let root = find(&mut parents, corner) as usize;
            if root_point[root] == NO_POINT {
                let original = corner_point[corner as usize];
                root_point[root] = if point_used[original as usize] {
                    let copy = u32::try_from(geo.len() + sources.len())
                        .map_err(|_| GeoError::LimitExceeded)?;
                    sources.push(original);
                    copy
                } else {
                    point_used[original as usize] = true;
                    original
                };
            }
            ids[corner as usize] = root_point[root];
        }
        match cut_clashes(faces, sides, &mut partner, flips, bases, &ids, &side_from) {
            Clashes::None => return Ok((ids, sources)),
            Clashes::Cut => {}
            Clashes::Stuck => return Err(GeoError::InvalidTopology),
        }
    }
}

enum Clashes {
    None,
    Cut,
    Stuck,
}

/// Separates sides that share an edge they should not. The edge keeps one pair, or failing that one
/// face. Every other side comes away at its `lo` corner, by dropping the join on that corner's other
/// edge. That is the smallest cut that works: a pair that comes away stays joined to itself, so one
/// clash cannot shred its neighbours.
fn cut_clashes(
    faces: &[Face],
    sides: &[Side],
    partner: &mut [u32],
    flips: &[bool],
    bases: &[u32],
    ids: &[u32],
    side_from: &[u32],
) -> Clashes {
    // Group sides by output edge `(lo, hi)` -> `Vec<(runs_forward, side_index)>`.
    let mut by_edge: BTreeMap<(u32, u32), Vec<(bool, u32)>> = BTreeMap::new();
    for (index, side) in sides.iter().enumerate() {
        let (lo_corner, hi_corner) = side_corners(*side, faces, bases);
        let (a, b) = (ids[lo_corner as usize], ids[hi_corner as usize]);
        let (lo, hi, same_order) = match a.cmp(&b) {
            std::cmp::Ordering::Less => (a, b, true),
            _ => (b, a, false),
        };
        let forward = side.forward ^ flips[side.face as usize] ^ same_order;
        by_edge
            .entry((lo, hi))
            .or_default()
            .push((forward, index as u32));
    }
    let mut found = false;
    let mut cut = false;
    for group in by_edge.values() {
        let clean = group.len() == 1 || (group.len() == 2 && group[0].0 != group[1].0);
        if !clean {
            found = true;
            let kept = group
                .iter()
                .find(|&&(_, side_idx)| {
                    let other = partner[side_idx as usize];
                    group.iter().any(|&(_, candidate)| candidate == other)
                })
                .map_or(group[0].1, |&(_, side_idx)| side_idx);
            let kept_partner = partner[kept as usize];
            for &(_, side_idx) in group {
                if side_idx != kept && side_idx != kept_partner {
                    let side = sides[side_idx as usize];
                    cut |= drop_other_join(side, faces, partner, bases, side_from);
                }
            }
        }
    }
    match (found, cut) {
        (false, _) => Clashes::None,
        (true, true) => Clashes::Cut,
        (true, false) => Clashes::Stuck,
    }
}

/// Drops the join on the other edge at `side`'s `lo` corner, or on `side` itself if that has none.
/// Returns whether it dropped anything.
fn drop_other_join(
    side: Side,
    faces: &[Face],
    partner: &mut [u32],
    bases: &[u32],
    side_from: &[u32],
) -> bool {
    let base = bases[side.face as usize];
    let count = faces[side.face as usize].points.len() as u32;
    // A side runs from its corner to the next. So at `lo`, the other edge is the one before when the
    // side runs forward, and the one after when it runs backward.
    let other_corner = if side.forward {
        (side.corner + count - 1) % count
    } else {
        (side.corner + 1) % count
    };
    let other = side_from[(base + other_corner) as usize];
    let this = side_from[(base + side.corner) as usize];
    for target in [other, this] {
        let joined = partner[target as usize];
        if joined != NO_SIDE {
            partner[target as usize] = NO_SIDE;
            partner[joined as usize] = NO_SIDE;
            return true;
        }
    }
    false
}

/// The output faces, each with its final points and where its data comes from.
struct Built {
    faces: Vec<Vec<u32>>,
    prim_sources: Vec<usize>,
    vertex_sources: Vec<usize>,
    /// Where each face's corners begin in `vertex_sources`.
    vertex_starts: Vec<usize>,
    /// How many faces came from the input; the rest fill holes.
    kept_faces: usize,
}

fn assemble(faces: &[Face], flips: &[bool], bases: &[u32], ids: &[u32]) -> Built {
    let mut built = Built {
        faces: Vec::with_capacity(faces.len()),
        prim_sources: Vec::with_capacity(faces.len()),
        vertex_sources: Vec::with_capacity(ids.len()),
        vertex_starts: Vec::with_capacity(faces.len()),
        kept_faces: faces.len(),
    };
    for (index, face) in faces.iter().enumerate() {
        let count = face.points.len();
        let base = bases[index] as usize;
        let mut order: Vec<usize> = (0..count).collect();
        if flips[index] {
            order.reverse();
        }
        built.vertex_starts.push(built.vertex_sources.len());
        built
            .vertex_sources
            .extend(order.iter().map(|&k| face.vertices[k]));
        built
            .faces
            .push(order.iter().map(|&k| ids[base + k]).collect());
        built.prim_sources.push(face.prim);
    }
    built
}

/// The start of a boundary edge, and the face corner it leaves from.
#[derive(Clone, Copy)]
struct BoundaryEdge {
    from: u32,
    to: u32,
    prim: usize,
    vertex: usize,
}

fn edge_key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

/// Traces the boundary into loops, filling those within the limit.
fn fill_holes(
    built: &mut Built,
    positions: &[D3],
    sources: &[u32],
    max_hole_edges: usize,
    report: &mut Report,
    ctx: &SopContext<'_>,
) -> Result<(), GeoError> {
    let mut boundary = find_boundary(built);
    boundary.sort_unstable_by_key(|edge| edge.from);
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    let mut triangles: HashSet<[u32; 3]> = HashSet::new();
    for face in &built.faces {
        for k in 0..face.len() {
            edges.insert(edge_key(face[k], face[(k + 1) % face.len()]));
        }
        if let [a, b, c] = face[..] {
            let mut key = [a, b, c];
            key.sort_unstable();
            triangles.insert(key);
        }
    }
    let original_count = positions.len();
    let position_of = |point: u32| -> D3 {
        let index = point as usize;
        if index < original_count {
            positions[index]
        } else {
            positions[sources[index - original_count] as usize]
        }
    };
    let limit = max_hole_edges.min(MAX_FILLED_HOLE_EDGES);
    let mut visited = vec![false; boundary.len()];
    for start in 0..boundary.len() {
        ctx.poll_every(start)?;
        if visited[start] {
            continue;
        }
        let mut chain = Vec::new();
        let mut at = start;
        let mut closed = false;
        while !visited[at] {
            visited[at] = true;
            chain.push(boundary[at]);
            let target = boundary[at].to;
            match boundary.binary_search_by_key(&target, |edge| edge.from) {
                Ok(next) => at = next,
                Err(_) => break,
            }
            closed = at == start;
        }
        let filled = closed
            && chain.len() <= limit
            && fill_one_hole(built, &chain, &position_of, &mut edges, &triangles, ctx)?;
        if filled {
            report.filled_holes += 1;
        } else {
            report.open_holes += 1;
        }
    }
    report.filled_faces = built.faces.len() - built.kept_faces;
    Ok(())
}

fn find_boundary(built: &Built) -> Vec<BoundaryEdge> {
    // (lower point, upper point, face, corner)
    let mut uses: Vec<(u32, u32, u32, u32)> = Vec::new();
    for (index, face) in built.faces.iter().enumerate() {
        for k in 0..face.len() {
            let (a, b) = edge_key(face[k], face[(k + 1) % face.len()]);
            uses.push((a, b, index as u32, k as u32));
        }
    }
    uses.sort_unstable();
    let mut boundary = Vec::new();
    for group in uses.chunk_by(|a, b| (a.0, a.1) == (b.0, b.1)) {
        if let [only] = group {
            let (face, corner) = (only.2 as usize, only.3 as usize);
            let points = &built.faces[face];
            boundary.push(BoundaryEdge {
                from: points[corner],
                to: points[(corner + 1) % points.len()],
                prim: built.prim_sources[face],
                vertex: built.vertex_sources[built.vertex_starts[face] + corner],
            });
        }
    }
    boundary
}

/// Fills one closed boundary with the triangles of least total area, using no edge the mesh already
/// has. Returns whether that was possible.
fn fill_one_hole(
    built: &mut Built,
    chain: &[BoundaryEdge],
    position_of: &impl Fn(u32) -> D3,
    edges: &mut HashSet<(u32, u32)>,
    triangles: &HashSet<[u32; 3]>,
    ctx: &SopContext<'_>,
) -> Result<bool, GeoError> {
    let count = chain.len();
    if let [a, b, c] = chain {
        // A lone triangle's boundary would only be filled by its own back.
        let mut key = [a.from, b.from, c.from];
        key.sort_unstable();
        if triangles.contains(&key) {
            return Ok(false);
        }
    }
    let area_cells = count.checked_mul(count).ok_or(GeoError::LimitExceeded)?;
    ctx.check_output(area_cells, area_cells)?;
    let points: Vec<D3> = chain.iter().map(|edge| position_of(edge.from)).collect();
    let usable = |i: usize, k: usize, edges: &HashSet<(u32, u32)>| {
        (i == 0 && k == count - 1) || !edges.contains(&edge_key(chain[i].from, chain[k].from))
    };
    // cost[i][k] is the least area spanning corners i to k; choice holds the corner achieving it.
    let mut cost = vec![0.0f64; area_cells];
    let mut choice = vec![usize::MAX; area_cells];
    for span in 2..count {
        ctx.poll()?;
        for i in 0..count - span {
            let k = i + span;
            let mut best = f64::INFINITY;
            let mut best_middle = usize::MAX;
            if usable(i, k, edges) {
                for middle in i + 1..k {
                    let area = length(cross(
                        sub(points[middle], points[i]),
                        sub(points[k], points[i]),
                    ));
                    let total = cost[i * count + middle] + cost[middle * count + k] + area;
                    if total < best {
                        best = total;
                        best_middle = middle;
                    }
                }
            }
            cost[i * count + k] = best;
            choice[i * count + k] = best_middle;
        }
    }
    if !cost[count - 1].is_finite() {
        return Ok(false);
    }
    let mut stack = vec![(0usize, count - 1)];
    for _ in 0..area_cells {
        let Some((i, k)) = stack.pop() else {
            break;
        };
        if k <= i + 1 {
            continue;
        }
        let middle = choice[i * count + k];
        // The new face runs against the boundary, which runs with the faces beside it.
        let corners = [middle, i, k];
        built.vertex_starts.push(built.vertex_sources.len());
        built
            .vertex_sources
            .extend(corners.iter().map(|&at| chain[at].vertex));
        built
            .faces
            .push(corners.iter().map(|&at| chain[at].from).collect());
        built.prim_sources.push(chain[i].prim);
        edges.insert(edge_key(chain[i].from, chain[k].from));
        stack.push((middle, k));
        stack.push((i, middle));
    }
    Ok(true)
}

/// Builds the result, carrying every attribute and group to the elements made from its owner.
fn build_output(
    geo: &GeoDetail,
    built: &Built,
    sources: &[u32],
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    let point_sources: Vec<usize> = (0..geo.len())
        .chain(sources.iter().map(|&source| source as usize))
        .collect();
    ctx.check_output(point_sources.len(), built.vertex_sources.len())?;
    let positions = point_sources
        .iter()
        .map(|&source| geo.point(source).ok_or(GeoError::IndexOutOfBounds))
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = GeoDetail::points(positions);
    for name in geo.domain_attribute_names(AttrDomain::Point) {
        if let Some(column) = geo.domain_attribute(AttrDomain::Point, name) {
            out = out.with_attribute(name, column.selected(&point_sources)?)?;
        }
    }
    for name in geo.domain_attribute_names(AttrDomain::Detail) {
        if let Some(column) = geo.domain_attribute(AttrDomain::Detail, name) {
            out = out.with_domain_attribute(AttrDomain::Detail, name, column.clone())?;
        }
    }
    let prims: Vec<(PrimKind, Vec<u32>)> = built
        .faces
        .iter()
        .map(|face| (PrimKind::Polygon, face.clone()))
        .collect();
    out = attach_prims(&out, &prims)?;
    for name in geo.domain_attribute_names(AttrDomain::Prim) {
        if let Some(column) = geo.domain_attribute(AttrDomain::Prim, name) {
            out = out.with_domain_attribute(
                AttrDomain::Prim,
                name,
                column.selected(&built.prim_sources)?,
            )?;
        }
    }
    for name in geo.domain_attribute_names(AttrDomain::Vertex) {
        if let Some(column) = geo.domain_attribute(AttrDomain::Vertex, name) {
            out = out.with_domain_attribute(
                AttrDomain::Vertex,
                name,
                column.selected(&built.vertex_sources)?,
            )?;
        }
    }
    for name in geo.group_names(GroupDomain::Point) {
        if let Some(group) = geo.group(GroupDomain::Point, name) {
            let members: Vec<usize> = point_sources
                .iter()
                .enumerate()
                .filter(|&(_, &source)| group.contains(source))
                .map(|(index, _)| index)
                .collect();
            out = out.with_group(GroupDomain::Point, name, &members)?;
        }
    }
    for name in geo.group_names(GroupDomain::Prim) {
        if name == FILLED_GROUP {
            continue;
        }
        if let Some(group) = geo.group(GroupDomain::Prim, name) {
            let members: Vec<usize> = built
                .prim_sources
                .iter()
                .take(built.kept_faces)
                .enumerate()
                .filter(|&(_, &source)| group.contains(source))
                .map(|(index, _)| index)
                .collect();
            out = out.with_group(GroupDomain::Prim, name, &members)?;
        }
    }
    let filled: Vec<usize> = (built.kept_faces..built.faces.len()).collect();
    if !filled.is_empty() {
        out = out.with_group(GroupDomain::Prim, FILLED_GROUP, &filled)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcc_geo::PrimRun;
    use pcc_math::Vec3;
    use std::sync::atomic::AtomicBool;

    fn poly_geo(points: &[[f32; 3]], faces: &[&[u32]]) -> Result<GeoDetail, GeoError> {
        let positions = points
            .iter()
            .map(|p| Vec3::checked(p[0], p[1], p[2]))
            .collect::<Result<Vec<_>, _>>()?;
        let runs: Vec<PrimRun> = faces
            .iter()
            .map(|f| PrimRun {
                kind: PrimKind::Polygon,
                arity: f.len() as u32,
                prim_count: 1,
            })
            .collect();
        let verts: Vec<u32> = faces.iter().flat_map(|f| f.iter().copied()).collect();
        GeoDetail::points(positions).with_primitives(runs, verts)
    }

    #[test]
    fn clean_manifold_checks_both_edges_and_pinched_vertices() -> Result<(), GeoError> {
        let tet = poly_geo(
            &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            &[&[0, 2, 1], &[0, 1, 3], &[1, 2, 3], &[0, 3, 2]],
        )?;
        assert!(is_clean_manifold(&tet));

        // Three triangles sharing edge (0, 1): non-manifold edge, no pinched vertex.
        let three_fins = poly_geo(
            &[
                [0., 0., 0.],
                [1., 0., 0.],
                [0., 1., 0.],
                [0., -1., 0.],
                [0., 0., 1.],
            ],
            &[&[0, 1, 2], &[1, 0, 3], &[0, 1, 4]],
        )?;
        assert!(!is_clean_manifold(&three_fins));

        // Two tetrahedra meeting only at origin: all edges manifold, origin pinched.
        let pinched = poly_geo(
            &[
                [0., 0., 0.],
                [1., 0., 0.],
                [0., 1., 0.],
                [0., 0., 1.],
                [-1., 0., 0.],
                [0., -1., 0.],
                [0., 0., -1.],
            ],
            &[
                &[0, 2, 1],
                &[0, 1, 3],
                &[1, 2, 3],
                &[0, 3, 2],
                &[0, 5, 4],
                &[0, 4, 6],
                &[4, 5, 6],
                &[0, 6, 5],
            ],
        )?;
        assert!(!is_clean_manifold(&pinched));
        Ok(())
    }

    #[test]
    fn same_direction_and_orient_tie_preserve_first_face_winding() -> Result<(), GeoError> {
        assert!(same_direction(&[0, 1, 2, 3], &[2, 3, 0, 1]));
        assert!(!same_direction(&[0, 1, 2, 3], &[0, 3, 2, 1]));
        assert!(same_direction(&[0, 1, 2], &[1, 2, 0]));
        assert!(!same_direction(&[0, 1, 2], &[0, 2, 1]));

        // Two equal-area triangles sharing edge (0, 1) with conflicting windings: tie keeps
        // face 0's original winding (`flipped == kept` must NOT invert the patch).
        let geo = poly_geo(
            &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., -1., 0.]],
            &[&[0, 1, 2], &[0, 1, 3]],
        )?;
        let cancel = AtomicBool::new(false);
        let repaired = mesh_repair(&geo, 0, &SopContext::new(1 << 20, &cancel))?;
        assert_eq!(repaired.vertex_point(0), Some(0));
        assert_eq!(repaired.vertex_point(1), Some(1));
        assert_eq!(repaired.vertex_point(2), Some(2));
        Ok(())
    }

    #[test]
    fn match_brackets_and_pair_around_edge_wrap_and_prefer_same_patch() {
        // Four sides around edge (0, 1) in angular order: [F0, B1, F2, B3].
        // With inverted=false: B opens, F closes -> B1 pairs with F2, and B3 wraps to F0!
        let sides = vec![
            Side {
                lo: 0,
                hi: 1,
                forward: true,
                face: 0,
                corner: 0,
            },
            Side {
                lo: 0,
                hi: 1,
                forward: false,
                face: 1,
                corner: 0,
            },
            Side {
                lo: 0,
                hi: 1,
                forward: true,
                face: 2,
                corner: 0,
            },
            Side {
                lo: 0,
                hi: 1,
                forward: false,
                face: 3,
                corner: 0,
            },
        ];
        let order = [0, 1, 2, 3];
        let flips = [false; 4];
        let solid = match_brackets(&sides, &order, &flips, false);
        assert_eq!(solid, vec![(1, 2), (3, 0)]);
        let inverted = match_brackets(&sides, &order, &flips, true);
        assert_eq!(inverted, vec![(0, 1), (2, 3)]);
        // An unmatched opener at index 0 must NOT pair with itself on the second sweep.
        let unmatched = match_brackets(&sides, &[1, 3, 2], &flips, false);
        assert_eq!(unmatched, vec![(3, 2)]);

        // Now verify `pair_around_edge`: 4 triangles around the Z-axis (0,0,0)->(0,0,1)
        // at angles 0, 90, 180, 270 degrees.
        let positions: Vec<D3> = vec![
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
        ];
        let faces = vec![
            Face {
                prim: 0,
                points: vec![0, 1, 2],
                vertices: vec![0, 1, 2],
            },
            Face {
                prim: 1,
                points: vec![1, 0, 3],
                vertices: vec![3, 4, 5],
            },
            Face {
                prim: 2,
                points: vec![0, 1, 4],
                vertices: vec![6, 7, 8],
            },
            Face {
                prim: 3,
                points: vec![1, 0, 5],
                vertices: vec![9, 10, 11],
            },
        ];
        // When patches match `inverted` ((0,1) same patch, (2,3) same patch), choose inverted.
        let oriented_inv = Oriented {
            flips: vec![false; 4],
            patches: vec![0, 0, 1, 1],
            conflicts: vec![false],
        };
        let mut pairs_inv = Vec::new();
        pair_around_edge(
            &faces,
            &sides,
            0..4,
            &oriented_inv,
            &positions,
            &mut pairs_inv,
        );
        assert_eq!(pairs_inv.len(), 2);
        let pair_patches: Vec<(u32, u32)> = pairs_inv
            .iter()
            .map(|&(a, b)| {
                (
                    oriented_inv.patches[a as usize],
                    oriented_inv.patches[b as usize],
                )
            })
            .collect();
        assert!(pair_patches.iter().all(|&(pa, pb)| pa == pb));

        // On an exact tie (all distinct patches: 0 vs 0 within-patch pairs), choose `solid`.
        let oriented_tie = Oriented {
            flips: vec![false; 4],
            patches: vec![0, 1, 2, 3],
            conflicts: vec![false],
        };
        let mut pairs_tie = Vec::new();
        pair_around_edge(
            &faces,
            &sides,
            0..4,
            &oriented_tie,
            &positions,
            &mut pairs_tie,
        );
        assert_eq!(pairs_tie.len(), 2);
        assert_ne!(pairs_tie, pairs_inv);
    }

    #[test]
    fn cut_clashes_and_drop_other_join_resolve_unpaired_shared_edges() {
        // Put the UNPAIRED clashing face FIRST (face 0: [0,1,4]) and the PAIRED manifold faces
        // after it (face 1: [0,1,2] and face 2: [1,0,3]) so `cut_clashes` must locate the paired
        // faces rather than falling back to `group[0]`.
        let faces = vec![
            Face {
                prim: 0,
                points: vec![0, 1, 4],
                vertices: vec![0, 1, 2],
            },
            Face {
                prim: 1,
                points: vec![0, 1, 2],
                vertices: vec![3, 4, 5],
            },
            Face {
                prim: 2,
                points: vec![1, 0, 3],
                vertices: vec![6, 7, 8],
            },
        ];
        let bases = vec![0, 3, 6];
        let sides = build_sides(&faces);
        let mut side_from = vec![NO_SIDE; 9];
        for (index, side) in sides.iter().enumerate() {
            side_from[(bases[side.face as usize] + side.corner) as usize] = index as u32;
        }
        let s_f0_01 = side_from[0];
        let s_f0_40 = side_from[2];
        let s_f1_01 = side_from[3];
        let s_f1_20 = side_from[5];
        let s_f2_10 = side_from[6];
        let s_f2_03 = side_from[7];
        let s_f2_31 = side_from[8];

        // Partner (f1, f2) on edge (0,1), partner f0's edge (4,0) with an unrelated edge (f2's
        // (3,1)), and partner f1's edge (2,0) with f2's (0,3) so cutting `kept` instead of f0
        // would cut a completely different edge.
        let mut partner = vec![NO_SIDE; sides.len()];
        partner[s_f1_01 as usize] = s_f2_10;
        partner[s_f2_10 as usize] = s_f1_01;
        partner[s_f0_40 as usize] = s_f2_31;
        partner[s_f2_31 as usize] = s_f0_40;
        partner[s_f1_20 as usize] = s_f2_03;
        partner[s_f2_03 as usize] = s_f1_20;

        let ids = vec![0, 1, 4, 0, 1, 2, 1, 0, 3];
        let flips = vec![false; 3];
        let result = cut_clashes(
            &faces,
            &sides,
            &mut partner,
            &flips,
            &bases,
            &ids,
            &side_from,
        );
        assert!(matches!(result, Clashes::Cut));
        // The paired manifold edge (f1, f2) and f1's own join stayed intact; f0's join was cut!
        assert_eq!(partner[s_f1_01 as usize], s_f2_10);
        assert_eq!(partner[s_f2_10 as usize], s_f1_01);
        assert_eq!(partner[s_f1_20 as usize], s_f2_03);
        assert_eq!(partner[s_f0_40 as usize], NO_SIDE);
        assert_eq!(partner[s_f2_31 as usize], NO_SIDE);

        // Two-face clean edge vs two-face same-winding clash where `ids` reverses `(a, b)` order
        // on one face (`a < b` vs `a > b`).
        let two_faces = vec![
            Face {
                prim: 0,
                points: vec![0, 1, 2],
                vertices: vec![0, 1, 2],
            },
            Face {
                prim: 1,
                points: vec![1, 0, 3],
                vertices: vec![3, 4, 5],
            },
        ];
        let two_bases = vec![0, 3];
        let two_sides = build_sides(&two_faces);
        let mut two_side_from = vec![NO_SIDE; 6];
        for (index, side) in two_sides.iter().enumerate() {
            two_side_from[(two_bases[side.face as usize] + side.corner) as usize] = index as u32;
        }
        let mut two_partner = vec![NO_SIDE; two_sides.len()];
        // Corner IDs give a=5, b=2 (`a > b`) on face 0 and a=2, b=5 (`a < b`) on face 1:
        // face 0 runs 5->2 (backward) and face 1 runs 5->2 (backward) — SAME direction = CLASH!
        // Because neither face has any join, `cut_clashes` must detect the clash (`Stuck`).
        let same_dir_clash_ids = vec![5, 2, 7, 5, 2, 8];
        assert!(matches!(
            cut_clashes(
                &two_faces,
                &two_sides,
                &mut two_partner,
                &[false, false],
                &two_bases,
                &same_dir_clash_ids,
                &two_side_from,
            ),
            Clashes::Stuck
        ));
        // Opposite windings on the same reversed IDs: face 0 runs 5->2, face 1 runs 2->5 = CLEAN!
        let opposite_dir_ids = vec![5, 2, 7, 2, 5, 8];
        assert!(matches!(
            cut_clashes(
                &two_faces,
                &two_sides,
                &mut two_partner,
                &[false, false],
                &two_bases,
                &opposite_dir_ids,
                &two_side_from,
            ),
            Clashes::None
        ));

        // Test `drop_other_join` directly for a backward side (`forward == false`), for fallback
        // to `this` at a non-zero corner (`base = 6, corner = 1`: `6+1 != 6-1 != 6*1`), and for
        // returning `false` when neither edge is joined.
        partner.fill(NO_SIDE);
        partner[s_f2_03 as usize] = s_f1_20;
        partner[s_f1_20 as usize] = s_f2_03;
        assert!(drop_other_join(
            sides[s_f2_10 as usize],
            &faces,
            &mut partner,
            &bases,
            &side_from,
        ));
        assert_eq!(partner[s_f2_03 as usize], NO_SIDE);

        // Fallback to `this` on corner 1 (`s_f2_03`, where `base = 6, corner = 1`).
        partner[s_f2_03 as usize] = s_f0_01;
        partner[s_f0_01 as usize] = s_f2_03;
        assert!(drop_other_join(
            sides[s_f2_03 as usize],
            &faces,
            &mut partner,
            &bases,
            &side_from,
        ));
        assert_eq!(partner[s_f2_03 as usize], NO_SIDE);

        // With neither joined, returns false (which turns into `Clashes::Stuck`).
        assert!(!drop_other_join(
            sides[s_f2_31 as usize],
            &faces,
            &mut partner,
            &bases,
            &side_from,
        ));
        assert!(matches!(
            cut_clashes(
                &faces,
                &sides,
                &mut partner,
                &flips,
                &bases,
                &ids,
                &side_from
            ),
            Clashes::Stuck
        ));
    }

    #[test]
    fn fill_holes_handles_split_points_and_minimum_area_pentagon() -> Result<(), GeoError> {
        let cancel = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 24, &cancel);

        // 1. Direct `fill_holes` test with a split point at `index = original_count + 2`
        //    (`6 - 4 = 2`, whereas `6 / 4 = 1`): corner 2 of the chevron hole is split point 6
        //    (source 2 = (0.2, 0.2, 0), giving diagonal (0, 6) and area 0.4; source 1 = (2, 0, 0)
        //    would flip the optimal diagonal to (1, 3)).
        let split_positions: Vec<D3> = vec![
            [0.0, 0.0, 0.0], // 0
            [2.0, 0.0, 0.0], // 1
            [0.2, 0.2, 0.0], // 2
            [0.0, 2.0, 0.0], // 3
        ];
        // A single reversed quad face [0, 3, 6, 1] leaves one boundary loop 0 -> 1 -> 6 -> 3 -> 0
        // (using split point 6 -> sources[6 - 4] = sources[2] = 2).
        let split_sources = vec![0u32, 1u32, 2u32];
        let mut split_built = Built {
            faces: vec![vec![0, 3, 6, 1]],
            prim_sources: vec![0],
            vertex_sources: vec![0, 1, 2, 3],
            vertex_starts: vec![0],
            kept_faces: 1,
        };
        let mut split_report = Report::default();
        fill_holes(
            &mut split_built,
            &split_positions,
            &split_sources,
            8,
            &mut split_report,
            &ctx,
        )?;
        assert_eq!(split_report.filled_holes, 1);
        assert_eq!(split_built.faces.len(), 3);
        // Optimal diagonal on the chevron uses split point 6 paired with 0 (NOT 1 paired with 3).
        assert!(
            split_built.faces[1..]
                .iter()
                .all(|f| f.contains(&0) && f.contains(&6)),
            "expected diagonal (0, 6), got {:?}",
            &split_built.faces[1..]
        );

        // 2. Concave chevron quad where diagonal (0, 2) (`middle = 2`, area 0.4) strictly beats
        //    diagonal (1, 3) (`middle = 1`, area 3.6): kills `cost[...] + area` -> `*`.
        let chevron: Vec<D3> = vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [0.2, 0.2, 0.0],
            [0.0, 2.0, 0.0],
        ];
        let quad_chain: Vec<BoundaryEdge> = (0..4)
            .map(|i| BoundaryEdge {
                from: i as u32,
                to: ((i + 1) % 4) as u32,
                prim: 0,
                vertex: i,
            })
            .collect();
        let mut built_chevron = Built {
            faces: Vec::new(),
            prim_sources: Vec::new(),
            vertex_sources: Vec::new(),
            vertex_starts: Vec::new(),
            kept_faces: 0,
        };
        let mut chevron_edges: HashSet<(u32, u32)> = (0..4)
            .map(|i| edge_key(i as u32, ((i + 1) % 4) as u32))
            .collect();
        let triangles = HashSet::new();
        assert!(fill_one_hole(
            &mut built_chevron,
            &quad_chain,
            &|p| chevron[p as usize],
            &mut chevron_edges,
            &triangles,
            &ctx,
        )?);
        let chevron_area: f64 = built_chevron
            .faces
            .iter()
            .map(|f| {
                0.5 * length(cross(
                    sub(chevron[f[1] as usize], chevron[f[0] as usize]),
                    sub(chevron[f[2] as usize], chevron[f[0] as usize]),
                ))
            })
            .sum();
        assert!(
            (chevron_area - 0.4).abs() < 1e-9,
            "expected optimal chevron area 0.4, got {chevron_area}"
        );

        // 3. Flat unit square tie-break: `<` picks `middle = 1` first (`[1, 0, 3]`), while `<=`
        //    would overwrite with `middle = 2`.
        let square: Vec<D3> = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let mut built_sq = Built {
            faces: Vec::new(),
            prim_sources: Vec::new(),
            vertex_sources: Vec::new(),
            vertex_starts: Vec::new(),
            kept_faces: 0,
        };
        let mut sq_edges: HashSet<(u32, u32)> = (0..4)
            .map(|i| edge_key(i as u32, ((i + 1) % 4) as u32))
            .collect();
        assert!(fill_one_hole(
            &mut built_sq,
            &quad_chain,
            &|p| square[p as usize],
            &mut sq_edges,
            &triangles,
            &ctx,
        )?);
        assert_eq!(built_sq.faces[0], vec![1, 0, 3]);

        // 4. Unfillable 4-sided hole (both diagonals already present in `edges`): returns `false`.
        let mut built_unfillable = Built {
            faces: Vec::new(),
            prim_sources: Vec::new(),
            vertex_sources: Vec::new(),
            vertex_starts: Vec::new(),
            kept_faces: 0,
        };
        let mut all_edges: HashSet<(u32, u32)> = (0..4)
            .map(|i| edge_key(i as u32, ((i + 1) % 4) as u32))
            .collect();
        all_edges.insert(edge_key(0, 2));
        all_edges.insert(edge_key(1, 3));
        assert!(!fill_one_hole(
            &mut built_unfillable,
            &quad_chain,
            &|p| square[p as usize],
            &mut all_edges,
            &triangles,
            &ctx,
        )?);

        // 5. Blocking diagonal (2, 4) on a bent pentagon forces a higher-area triangulation.
        let pts: Vec<D3> = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 2.0],
            [0.0, 1.0, 0.0],
        ];
        let chain: Vec<BoundaryEdge> = (0..5)
            .map(|i| BoundaryEdge {
                from: i as u32,
                to: ((i + 1) % 5) as u32,
                prim: 0,
                vertex: i,
            })
            .collect();
        let mut built_blocked = Built {
            faces: Vec::new(),
            prim_sources: Vec::new(),
            vertex_sources: Vec::new(),
            vertex_starts: Vec::new(),
            kept_faces: 0,
        };
        let mut blocked_edges: HashSet<(u32, u32)> = (0..5)
            .map(|i| edge_key(i as u32, ((i + 1) % 5) as u32))
            .collect();
        blocked_edges.insert(edge_key(2, 4));
        assert!(fill_one_hole(
            &mut built_blocked,
            &chain,
            &|p| pts[p as usize],
            &mut blocked_edges,
            &triangles,
            &ctx,
        )?);
        let blocked_area: f64 = built_blocked
            .faces
            .iter()
            .map(|f| {
                0.5 * length(cross(
                    sub(pts[f[1] as usize], pts[f[0] as usize]),
                    sub(pts[f[2] as usize], pts[f[0] as usize]),
                ))
            })
            .sum();
        assert!(blocked_area > 2.1, "got {blocked_area}");

        // 6. Scaled 5-sided chevron-tent where `middle = 2` has both `cost[0..2] > 1` and
        //    `cost[2..4] > 1` (true total ~39.8, vs `m = 1` > 76), whereas replacing the first `+`
        //    with `*` inflates `m = 2` to `3.6 * 32.6 + 3.6 > 120` and drops `m = 1` to `36.0`.
        let scaled_pent: Vec<D3> = vec![
            [0.0, 0.0, 0.0],
            [6.0, 0.0, 0.0],
            [0.6, 0.6, 0.0],
            [0.3, 3.3, 6.0],
            [0.0, 6.0, 0.0],
        ];
        let mut built_pent = Built {
            faces: Vec::new(),
            prim_sources: Vec::new(),
            vertex_sources: Vec::new(),
            vertex_starts: Vec::new(),
            kept_faces: 0,
        };
        let mut pent_edges: HashSet<(u32, u32)> = (0..5)
            .map(|i| edge_key(i as u32, ((i + 1) % 5) as u32))
            .collect();
        assert!(fill_one_hole(
            &mut built_pent,
            &chain,
            &|p| scaled_pent[p as usize],
            &mut pent_edges,
            &triangles,
            &ctx,
        )?);
        let pent_area: f64 = built_pent
            .faces
            .iter()
            .map(|f| {
                0.5 * length(cross(
                    sub(scaled_pent[f[1] as usize], scaled_pent[f[0] as usize]),
                    sub(scaled_pent[f[2] as usize], scaled_pent[f[0] as usize]),
                ))
            })
            .sum();
        assert!(
            pent_area < 21.0,
            "expected optimal area ~19.9, got {pent_area}"
        );
        Ok(())
    }
}
