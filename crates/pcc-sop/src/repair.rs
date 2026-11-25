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
use std::collections::HashSet;
use std::ops::Range;

/// Largest hole that can be filled: the triangulation costs the cube of its edge count.
pub const MAX_FILLED_HOLE_EDGES: usize = 512;

/// Primitive group holding the faces that fill holes.
pub const FILLED_GROUP: &str = "repair_filled";

const NO_POINT: u32 = u32::MAX;
const NO_SIDE: u32 = u32::MAX;

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

    let topology = TopologyIndex::build(&out);
    if !topology.is_manifold() || topology.non_manifold_vertex_count(&out) != 0 {
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
    let mut start = 0usize;
    while start < order.len() {
        ctx.poll_every(start)?;
        let mut end = start + 1;
        while end < order.len() && keys[order[end]] == keys[order[start]] {
            end += 1;
        }
        let group = &order[start..end];
        if let [first, rest @ ..] = group
            && !rest.is_empty()
        {
            let same: Vec<bool> = group
                .iter()
                .map(|&face| same_direction(&faces[*first].points, &faces[face].points))
                .collect();
            let forward = same.iter().filter(|&&is_same| is_same).count();
            let backward = group.len() - forward;
            if forward == backward {
                report.back_to_back += group.len();
                group.iter().for_each(|&face| keep[face] = false);
            } else {
                report.duplicate += group.len() - 1;
                let majority = forward > backward;
                let survivor = same.iter().position(|&is_same| is_same == majority);
                for (at, &face) in group.iter().enumerate() {
                    keep[face] = Some(at) == survivor;
                }
            }
        }
        start = end;
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
            sides.push(Side {
                lo: from.min(to),
                hi: from.max(to),
                forward: from < to,
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
    let mut start = 0usize;
    while start < sides.len() {
        let mut end = start + 1;
        while end < sides.len()
            && sides[end].lo == sides[start].lo
            && sides[end].hi == sides[start].hi
        {
            end += 1;
        }
        groups.push(start..end);
        start = end;
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
    // Two sweeps, because a bracket can wrap past the angle where the sort began.
    for step in 0..2 * count {
        let at = step % count;
        if taken[at] {
            continue;
        }
        let side = sides[order[at]];
        if side.forward ^ flips[side.face as usize] ^ inverted {
            if let Some(opener) = open.pop() {
                taken[opener] = true;
                taken[at] = true;
                pairs.push((order[opener] as u32, order[at] as u32));
            }
        } else if step < count {
            open.push(at);
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

fn find(parents: &mut [u32], mut at: u32) -> u32 {
    while parents[at as usize] != at {
        let grandparent = parents[parents[at as usize] as usize];
        parents[at as usize] = grandparent;
        at = grandparent;
    }
    at
}

fn union(parents: &mut [u32], a: u32, b: u32) {
    let (a, b) = (find(parents, a), find(parents, b));
    if a < b {
        parents[b as usize] = a;
    } else {
        parents[a as usize] = b;
    }
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
            if other != NO_SIDE && (index as u32) < other {
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
    // (lower point, upper point, runs from lower to upper, index into sides)
    let mut uses: Vec<(u32, u32, bool, u32)> = sides
        .iter()
        .enumerate()
        .map(|(index, side)| {
            let (lo_corner, hi_corner) = side_corners(*side, faces, bases);
            let (a, b) = (ids[lo_corner as usize], ids[hi_corner as usize]);
            let lower_first = a < b;
            let forward = side.forward ^ flips[side.face as usize];
            (a.min(b), a.max(b), forward == lower_first, index as u32)
        })
        .collect();
    uses.sort_unstable_by_key(|entry| (entry.0, entry.1, entry.3));
    let mut found = false;
    let mut cut = false;
    let mut start = 0usize;
    while start < uses.len() {
        let mut end = start + 1;
        while end < uses.len() && uses[end].0 == uses[start].0 && uses[end].1 == uses[start].1 {
            end += 1;
        }
        let group = &uses[start..end];
        let clean = group.len() == 1 || (group.len() == 2 && group[0].2 != group[1].2);
        if !clean {
            found = true;
            let kept = group
                .iter()
                .find(|entry| {
                    let other = partner[entry.3 as usize];
                    group.iter().any(|candidate| candidate.3 == other)
                })
                .map_or(group[0].3, |entry| entry.3);
            let kept_partner = partner[kept as usize];
            for entry in group {
                if entry.3 != kept && entry.3 != kept_partner {
                    let side = sides[entry.3 as usize];
                    cut |= drop_other_join(side, faces, partner, bases, side_from);
                }
            }
        }
        start = end;
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
    let mut start = 0usize;
    while start < uses.len() {
        let mut end = start + 1;
        while end < uses.len() && uses[end].0 == uses[start].0 && uses[end].1 == uses[start].1 {
            end += 1;
        }
        if end == start + 1 {
            let (face, corner) = (uses[start].2 as usize, uses[start].3 as usize);
            let points = &built.faces[face];
            boundary.push(BoundaryEdge {
                from: points[corner],
                to: points[(corner + 1) % points.len()],
                prim: built.prim_sources[face],
                vertex: built.vertex_sources[built.vertex_starts[face] + corner],
            });
        }
        start = end;
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
        k == i + 1
            || (i == 0 && k == count - 1)
            || !edges.contains(&edge_key(chain[i].from, chain[k].from))
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
    while let Some((i, k)) = stack.pop() {
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
