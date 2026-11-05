//! UV flattening with least-squares conformal maps (Levy et al. 2002) and island packing.
//!
//! Charts start from dihedral-angle flood fill and are bisected until each is a topological disk
//! that flattens with no inverted triangle and bounded area distortion. A single triangle always
//! satisfies this, so termination and flip-free charts hold for any input. On difficult models the
//! price is fragmentation, down to one island per face. UVs are stored as `f32`, so a sliver's area
//! can round to zero or slightly below; it is never inverted by more than that rounding.

use crate::common::{
    D3, SopContext, add, any_perpendicular, cross, distance, dot, length, load_points,
    load_polygons, normalized, polygon_area_vector, scale, sub,
};
use pcc_geo::{AttrDomain, AttributeColumn, GeoDetail, GeoError, TopologyIndex};
use pcc_math::Vec3;
use std::collections::{BTreeMap, BTreeSet};

/// Largest accepted ratio between the most and least magnified triangle within one chart.
const MAX_AREA_DISTORTION: f64 = 64.0;

struct FaceSet {
    polygons: Vec<Vec<u32>>,
    /// First vertex (half-edge) index of each polygon.
    starts: Vec<u32>,
    normals: Vec<D3>,
    centroids: Vec<D3>,
    /// Half-edge to owning polygon.
    he_face: Vec<u32>,
}

fn load_faces(geo: &GeoDetail) -> Result<FaceSet, GeoError> {
    let points = load_points(geo)?;
    let polygons = load_polygons(geo)?;
    if polygons.len() != geo.prim_count() {
        return Err(GeoError::InvalidTopology);
    }
    let mut starts = Vec::with_capacity(polygons.len());
    let mut normals = Vec::with_capacity(polygons.len());
    let mut centroids = Vec::with_capacity(polygons.len());
    let mut he_face = Vec::with_capacity(geo.vertex_count());
    let mut cursor = 0u32;
    for (index, polygon) in polygons.iter().enumerate() {
        starts.push(cursor);
        cursor += polygon.len() as u32;
        normals.push(normalized(polygon_area_vector(&points, polygon)).unwrap_or([0.0; 3]));
        let mut centroid = [0.0; 3];
        for &p in polygon {
            centroid = add(centroid, points[p as usize]);
        }
        centroids.push(scale(centroid, 1.0 / polygon.len() as f64));
        he_face.extend(std::iter::repeat_n(index as u32, polygon.len()));
    }
    Ok(FaceSet {
        polygons,
        starts,
        normals,
        centroids,
        he_face,
    })
}

/// Splits `faces` into components connected through twin half-edges admitted by `joinable`.
fn components(
    faces: &FaceSet,
    topology: &TopologyIndex,
    subset: &[u32],
    joinable: impl Fn(u32, u32) -> bool,
) -> Vec<Vec<u32>> {
    let members: BTreeSet<u32> = subset.iter().copied().collect();
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for &seed in subset {
        if !seen.insert(seed) {
            continue;
        }
        let mut stack = vec![seed];
        let mut component = Vec::new();
        while let Some(face) = stack.pop() {
            component.push(face);
            let start = faces.starts[face as usize];
            for he in start..start + faces.polygons[face as usize].len() as u32 {
                if let Some(twin) = topology.twin(he) {
                    let other = faces.he_face[twin as usize];
                    if members.contains(&other) && joinable(face, other) && seen.insert(other) {
                        stack.push(other);
                    }
                }
            }
        }
        component.sort_unstable();
        result.push(component);
    }
    result
}

/// Whether a connected chart is a topological disk: `V - E + F = 1` with one boundary loop.
fn is_disk(faces: &FaceSet, topology: &TopologyIndex, chart: &[u32]) -> bool {
    let members: BTreeSet<u32> = chart.iter().copied().collect();
    let mut vertices = BTreeSet::new();
    let mut half_edges = 0usize;
    let mut interior = 0usize;
    let mut boundary_next: BTreeMap<u32, u32> = BTreeMap::new();
    for &face in chart {
        let polygon = &faces.polygons[face as usize];
        let start = faces.starts[face as usize];
        for (k, &p) in polygon.iter().enumerate() {
            vertices.insert(p);
            half_edges += 1;
            let he = start + k as u32;
            let inside = topology
                .twin(he)
                .is_some_and(|twin| members.contains(&faces.he_face[twin as usize]));
            if inside {
                interior += 1;
            } else {
                let to = polygon[(k + 1) % polygon.len()];
                if boundary_next.insert(p, to).is_some() {
                    return false; // Pinched boundary vertex.
                }
            }
        }
    }
    let edges = (half_edges + (half_edges - interior)) / 2;
    if vertices.len() as i64 - edges as i64 + chart.len() as i64 != 1 || boundary_next.is_empty() {
        return false;
    }
    // Exactly one boundary loop: walking from any boundary vertex must visit them all.
    let Some((&first, _)) = boundary_next.iter().next() else {
        return false;
    };
    let mut current = first;
    let mut visited = 0usize;
    loop {
        let Some(&next) = boundary_next.get(&current) else {
            return false;
        };
        visited += 1;
        current = next;
        if current == first {
            break;
        }
        if visited > boundary_next.len() {
            return false;
        }
    }
    visited == boundary_next.len()
}

struct Flattened {
    /// Chart-local point index by mesh point.
    local: BTreeMap<u32, u32>,
    uv: Vec<[f64; 2]>,
}

/// Row of the sparse LSCM system; `(variable, coefficient)` pairs.
type Row = Vec<(u32, f64)>;

/// Solves the normal equations of a rectangular system by Jacobi-preconditioned CG.
fn solve_least_squares(
    rows: &[Row],
    rhs: &[f64],
    variables: usize,
    ctx: &SopContext<'_>,
) -> Result<Vec<f64>, GeoError> {
    let apply = |x: &[f64], out: &mut [f64], scratch: &mut [f64]| {
        for (row, slot) in rows.iter().zip(scratch.iter_mut()) {
            *slot = row.iter().map(|&(j, c)| c * x[j as usize]).sum();
        }
        out.fill(0.0);
        for (row, &value) in rows.iter().zip(scratch.iter()) {
            for &(j, c) in row {
                out[j as usize] += c * value;
            }
        }
    };
    let mut diagonal = vec![0.0; variables];
    let mut b = vec![0.0; variables];
    for (row, &value) in rows.iter().zip(rhs) {
        for &(j, c) in row {
            diagonal[j as usize] += c * c;
            b[j as usize] += c * value;
        }
    }
    let inverse: Vec<f64> = diagonal
        .iter()
        .map(|&d| if d > 0.0 { 1.0 / d } else { 1.0 })
        .collect();
    let mut x = vec![0.0; variables];
    let mut r = b;
    let mut z: Vec<f64> = r.iter().zip(&inverse).map(|(a, m)| a * m).collect();
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, c)| a * c).sum();
    let initial = rz.abs();
    let mut ap = vec![0.0; variables];
    let mut scratch = vec![0.0; rows.len()];
    let limit = (variables * 10).max(200);
    for _ in 0..limit {
        ctx.poll()?;
        if rz.abs() <= 1e-20 * initial || rz == 0.0 {
            break;
        }
        apply(&p, &mut ap, &mut scratch);
        let denominator: f64 = p.iter().zip(&ap).map(|(a, c)| a * c).sum();
        if denominator <= 0.0 {
            break;
        }
        let alpha = rz / denominator;
        for ((xi, ri), (pi, api)) in x.iter_mut().zip(r.iter_mut()).zip(p.iter().zip(&ap)) {
            *xi += alpha * pi;
            *ri -= alpha * api;
        }
        for ((zi, ri), m) in z.iter_mut().zip(&r).zip(&inverse) {
            *zi = ri * m;
        }
        let next: f64 = r.iter().zip(&z).map(|(a, c)| a * c).sum();
        let beta = next / rz;
        rz = next;
        for (pi, zi) in p.iter_mut().zip(&z) {
            *pi = zi + beta * *pi;
        }
    }
    Ok(x)
}

/// Fan triangles (in chart-local indices) of every polygon of `chart`.
fn chart_triangles(faces: &FaceSet, chart: &[u32], local: &BTreeMap<u32, u32>) -> Vec<[u32; 3]> {
    let mut triangles = Vec::new();
    let map = |p: u32| local.get(&p).copied().unwrap_or(0);
    for &face in chart {
        let polygon = &faces.polygons[face as usize];
        for k in 1..polygon.len() - 1 {
            triangles.push([map(polygon[0]), map(polygon[k]), map(polygon[k + 1])]);
        }
    }
    triangles
}

/// Conformal parameterization of one disk chart, pinning two distant boundary points.
fn lscm(
    faces: &FaceSet,
    chart: &[u32],
    points: &[D3],
    ctx: &SopContext<'_>,
) -> Result<Flattened, GeoError> {
    let mut local = BTreeMap::new();
    let mut positions: Vec<D3> = Vec::new();
    for &face in chart {
        for &p in &faces.polygons[face as usize] {
            local.entry(p).or_insert_with(|| {
                positions.push(points[p as usize]);
                (positions.len() - 1) as u32
            });
        }
    }
    let triangles = chart_triangles(faces, chart, &local);
    let n = positions.len();
    // Two approximately most distant points fix translation, rotation and scale.
    let farthest_from = |origin: D3| {
        (0..n)
            .max_by(|&a, &b| {
                distance(positions[a], origin)
                    .total_cmp(&distance(positions[b], origin))
                    .then(b.cmp(&a))
            })
            .unwrap_or(0)
    };
    let pin_a = farthest_from(positions[0]);
    let pin_b = farthest_from(positions[pin_a]);
    let pinned_span = distance(positions[pin_a], positions[pin_b]);
    if pin_a == pin_b || pinned_span <= 0.0 {
        return Err(GeoError::InvalidTopology);
    }
    let pin_values: [(usize, [f64; 2]); 2] = [(pin_a, [0.0, 0.0]), (pin_b, [pinned_span, 0.0])];

    // Free variables: u then v for each non-pinned point. Pinned terms move to the right side.
    let mut slots = vec![u32::MAX; n];
    let mut free = 0u32;
    for (i, slot) in slots.iter_mut().enumerate() {
        if i != pin_a && i != pin_b {
            *slot = free;
            free += 1;
        }
    }
    let variables = free as usize * 2;
    let mut rows: Vec<Row> = Vec::with_capacity(triangles.len() * 2);
    let mut rhs = Vec::with_capacity(triangles.len() * 2);
    for t in &triangles {
        let (p0, p1, p2) = (
            positions[t[0] as usize],
            positions[t[1] as usize],
            positions[t[2] as usize],
        );
        let e1 = sub(p1, p0);
        let e2 = sub(p2, p0);
        let area2 = length(cross(e1, e2));
        if area2 <= 1e-20 {
            continue;
        }
        // Isometric local frame of the triangle.
        let x_axis = scale(e1, 1.0 / length(e1));
        let (x1, x2, y2) = (length(e1), dot(e2, x_axis), area2 / length(e1));
        // Complex gradient weights W_j = (x_{j+2} - x_{j+1}) + i (y_{j+2} - y_{j+1}).
        let weights = [(x2 - x1, y2), (-x2, -y2), (x1, 0.0)];
        let norm = 1.0 / area2.sqrt();
        let mut real_row: Row = Vec::with_capacity(6);
        let mut imag_row: Row = Vec::with_capacity(6);
        let (mut real_rhs, mut imag_rhs) = (0.0, 0.0);
        for (k, &(wr, wi)) in weights.iter().enumerate() {
            let vertex = t[k] as usize;
            let (wr, wi) = (wr * norm, wi * norm);
            let pinned = pin_values.iter().find(|&&(i, _)| i == vertex);
            if let Some(&(_, [u, v])) = pinned {
                real_rhs -= wr * u - wi * v;
                imag_rhs -= wi * u + wr * v;
            } else {
                let slot = slots[vertex];
                let (u_var, v_var) = (slot * 2, slot * 2 + 1);
                real_row.push((u_var, wr));
                real_row.push((v_var, -wi));
                imag_row.push((u_var, wi));
                imag_row.push((v_var, wr));
            }
        }
        rows.push(real_row);
        rhs.push(real_rhs);
        rows.push(imag_row);
        rhs.push(imag_rhs);
    }
    let solution = solve_least_squares(&rows, &rhs, variables, ctx)?;
    let mut uv = vec![[0.0; 2]; n];
    for (i, slot) in slots.iter().enumerate() {
        if *slot != u32::MAX {
            uv[i] = [
                solution[*slot as usize * 2],
                solution[*slot as usize * 2 + 1],
            ];
        }
    }
    for (i, value) in pin_values {
        uv[i] = value;
    }
    Ok(Flattened { local, uv })
}

fn signed_uv_area(uv: &[[f64; 2]], t: &[u32; 3]) -> f64 {
    let (a, b, c) = (uv[t[0] as usize], uv[t[1] as usize], uv[t[2] as usize]);
    ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) * 0.5
}

/// Checks that no triangle is inverted or degenerate and bounds the area distortion, then applies
/// the shared texel density. Returns `false` when the chart must be subdivided further. Each row
/// of the LSCM system is built in the triangle's own positively oriented frame, so its minimizer
/// is orientation-preserving: only isolated flips can occur, never a mirrored chart.
fn accept_chart(flat: &mut Flattened, faces: &FaceSet, chart: &[u32], points: &[D3]) -> bool {
    // Area magnification per polygon, compared across the chart.
    let (mut smallest, mut largest) = (f64::INFINITY, 0.0f64);
    let (mut uv_total, mut mesh_total) = (0.0, 0.0);
    for &face in chart {
        let polygon = &faces.polygons[face as usize];
        let mesh_area = length(polygon_area_vector(points, polygon)) * 0.5;
        let mut uv_area = 0.0;
        for k in 1..polygon.len() - 1 {
            let tri = [
                flat.local.get(&polygon[0]).copied().unwrap_or(0),
                flat.local.get(&polygon[k]).copied().unwrap_or(0),
                flat.local.get(&polygon[k + 1]).copied().unwrap_or(0),
            ];
            let area = signed_uv_area(&flat.uv, &tri);
            if area <= 0.0 || !area.is_finite() {
                return false;
            }
            uv_area += area;
        }
        uv_total += uv_area;
        mesh_total += mesh_area;
        if mesh_area > 0.0 {
            let magnification = uv_area / mesh_area;
            smallest = smallest.min(magnification);
            largest = largest.max(magnification);
        }
    }
    if chart.len() > 1 && largest > MAX_AREA_DISTORTION * smallest {
        return false;
    }
    // Equalize texel density: one unit of UV area per unit of surface area.
    if uv_total > 0.0 && mesh_total > 0.0 {
        let factor = (mesh_total / uv_total).sqrt();
        for value in &mut flat.uv {
            value[0] *= factor;
            value[1] *= factor;
        }
    }
    true
}

/// Projects one polygon isometrically onto its own plane, so that a triangle always has positive
/// area. A polygon with no area at all gets a tiny counter-clockwise placeholder, which lets the
/// attribute cover every vertex without implying a real parameterization.
fn flatten_single_face(points: &[D3], polygon: &[u32]) -> Vec<[f64; 2]> {
    let Some(normal) = normalized(polygon_area_vector(points, polygon)) else {
        let step = std::f64::consts::TAU / polygon.len() as f64;
        return (0..polygon.len())
            .map(|k| {
                let (sin, cos) = (step * k as f64).sin_cos();
                [1e-6 * cos, 1e-6 * sin]
            })
            .collect();
    };
    let u = any_perpendicular(normal);
    let v = cross(normal, u);
    polygon
        .iter()
        .map(|&p| [dot(points[p as usize], u), dot(points[p as usize], v)])
        .collect()
}

/// Divides a chart along the longest axis of its face centroids.
fn bisect(faces: &FaceSet, chart: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for &face in chart {
        let c = faces.centroids[face as usize];
        for (axis, &value) in c.iter().enumerate() {
            low[axis] = low[axis].min(value);
            high[axis] = high[axis].max(value);
        }
    }
    let spans = sub(high, low);
    let axis = if spans[0] >= spans[1] && spans[0] >= spans[2] {
        0
    } else if spans[1] >= spans[2] {
        1
    } else {
        2
    };
    let mut sorted = chart.to_vec();
    sorted.sort_by(|&a, &b| {
        faces.centroids[a as usize][axis]
            .total_cmp(&faces.centroids[b as usize][axis])
            .then(a.cmp(&b))
    });
    let right = sorted.split_off(sorted.len() / 2);
    (sorted, right)
}

/// One island's corner UVs, keyed by half-edge.
struct Island {
    corners: Vec<(u32, [f64; 2])>,
    faces: Vec<u32>,
}

/// A placed rectangle, in units where each UDIM tile spans `[0, 1]`.
#[derive(Clone, Copy)]
struct Placement {
    x: f64,
    y: f64,
    tile: u32,
}

/// Bottom-left skyline packing into `tiles` unit squares. Returns `None` when something misses.
fn skyline_pack(sizes: &[(f64, f64)], tiles: u32) -> Option<Vec<Placement>> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|&a, &b| sizes[b].1.total_cmp(&sizes[a].1).then(a.cmp(&b)));
    let mut placements = vec![
        Placement {
            x: 0.0,
            y: 0.0,
            tile: 0,
        };
        sizes.len()
    ];
    // Each skyline is a list of `(x, height)` steps, sorted by x and starting at zero.
    let mut skylines: Vec<Vec<(f64, f64)>> = vec![vec![(0.0, 0.0)]; tiles as usize];
    for index in order {
        let (w, h) = sizes[index];
        if w > 1.0 || h > 1.0 {
            return None;
        }
        let mut best: Option<(f64, f64, usize, usize)> = None;
        for (tile, skyline) in skylines.iter().enumerate() {
            for (step, &(x, _)) in skyline.iter().enumerate() {
                if x + w > 1.0 {
                    break;
                }
                let top = skyline[step..]
                    .iter()
                    .take_while(|&&(sx, _)| sx < x + w)
                    .map(|&(_, height)| height)
                    .fold(0.0f64, f64::max);
                if top + h <= 1.0 && best.is_none_or(|b| (top, x) < (b.0, b.1)) {
                    best = Some((top, x, tile, step));
                }
            }
            if best.is_some() {
                break; // Prefer filling earlier tiles first.
            }
        }
        let (top, x, tile, _) = best?;
        placements[index] = Placement {
            x,
            y: top,
            tile: tile as u32,
        };
        let skyline = &mut skylines[tile];
        let end = x + w;
        let resume = skyline
            .iter()
            .take_while(|&&(sx, _)| sx <= end)
            .last()
            .map_or(0.0, |&(_, height)| height);
        skyline.retain(|&(sx, _)| sx < x || sx > end);
        skyline.push((x, top + h));
        if end < 1.0 {
            skyline.push((end, resume));
        }
        skyline.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    Some(placements)
}

/// Packs islands at the largest uniform scale that fits, keeping relative texel density, and
/// writes vertex `v@uv` and primitive `i@uv_island`.
fn pack_islands(
    geo: &GeoDetail,
    islands: &[Island],
    padding: f64,
    tiles: u32,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    let mut corners = vec![[0.0f64; 2]; geo.vertex_count()];
    let mut island_of = vec![0i32; geo.prim_count()];
    if islands.is_empty() {
        return Ok(geo.clone());
    }
    let mut sizes = Vec::with_capacity(islands.len());
    let mut origins = Vec::with_capacity(islands.len());
    let mut area = 0.0;
    for island in islands {
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for (_, uv) in &island.corners {
            for (axis, &value) in uv.iter().enumerate() {
                lo[axis] = lo[axis].min(value);
                hi[axis] = hi[axis].max(value);
            }
        }
        let size = ((hi[0] - lo[0]).max(1e-12), (hi[1] - lo[1]).max(1e-12));
        area += size.0 * size.1;
        sizes.push(size);
        origins.push(lo);
    }
    let attempt = |s: f64| {
        let padded: Vec<(f64, f64)> = sizes
            .iter()
            .map(|&(w, h)| (w * s + padding, h * s + padding))
            .collect();
        skyline_pack(&padded, tiles)
    };
    // Halve down from the area upper bound until something fits, then bisect the bracket.
    let mut too_large = (f64::from(tiles) / area).sqrt();
    let mut best = None;
    for _ in 0..64 {
        ctx.poll()?;
        if let Some(found) = attempt(too_large) {
            best = Some((too_large, found));
            break;
        }
        too_large *= 0.5;
    }
    let (mut factor, mut placements) = best.ok_or(GeoError::LimitExceeded)?;
    too_large = factor * 2.0;
    for _ in 0..24 {
        ctx.poll()?;
        let middle = 0.5 * (factor + too_large);
        match attempt(middle) {
            Some(found) => {
                factor = middle;
                placements = found;
            }
            None => too_large = middle,
        }
    }
    for (index, island) in islands.iter().enumerate() {
        let place = placements[index];
        let (tile_u, tile_v) = (f64::from(place.tile % 10), f64::from(place.tile / 10));
        for &(he, uv) in &island.corners {
            corners[he as usize] = [
                tile_u + place.x + padding * 0.5 + (uv[0] - origins[index][0]) * factor,
                tile_v + place.y + padding * 0.5 + (uv[1] - origins[index][1]) * factor,
            ];
        }
        for &face in &island.faces {
            island_of[face as usize] = index as i32;
        }
    }
    let values = corners
        .iter()
        .map(|c| Vec3::checked(c[0] as f32, c[1] as f32, 0.0))
        .collect::<Result<Vec<_>, _>>()?;
    geo.with_domain_attribute(
        AttrDomain::Vertex,
        "uv",
        AttributeColumn::vec3_values(values),
    )?
    .with_domain_attribute(
        AttrDomain::Prim,
        "uv_island",
        AttributeColumn::i32_values(island_of),
    )
}

/// Flattens polygons into non-overlapping UV islands in the unit square: vertex `v@uv` and
/// primitive `i@uv_island`. Faces meeting at more than `seam_angle_degrees` start in separate
/// charts. Output triangles have positive UV area up to `f32` rounding of their coordinates, and
/// islands share one texel density.
/// Polylines are not supported; existing attributes stay attached.
pub fn uv_flatten(
    geo: &GeoDetail,
    seam_angle_degrees: f64,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if !seam_angle_degrees.is_finite() || seam_angle_degrees < 0.0 {
        return Err(GeoError::InvalidTopology);
    }
    ctx.check_output(geo.vertex_count() * 4, geo.vertex_count() * 8)?;
    let faces = load_faces(geo)?;
    let points = load_points(geo)?;
    let topology = TopologyIndex::build(geo);
    let threshold = seam_angle_degrees.to_radians().cos();
    let all: Vec<u32> = (0..faces.polygons.len() as u32).collect();
    let mut pending = components(&faces, &topology, &all, |a, b| {
        dot(faces.normals[a as usize], faces.normals[b as usize]) >= threshold
    });
    pending.reverse();
    let mut islands = Vec::new();
    let mut rounds = 0usize;
    while let Some(chart) = pending.pop() {
        rounds += 1;
        ctx.poll_every(rounds)?;
        if let [face] = chart[..] {
            let start = faces.starts[face as usize];
            let corners = flatten_single_face(&points, &faces.polygons[face as usize])
                .into_iter()
                .enumerate()
                .map(|(k, uv)| (start + k as u32, uv))
                .collect();
            islands.push(Island {
                corners,
                faces: chart,
            });
            continue;
        }
        let disk = is_disk(&faces, &topology, &chart);
        let mut flattened = None;
        if disk {
            let mut candidate = lscm(&faces, &chart, &points, ctx)?;
            if accept_chart(&mut candidate, &faces, &chart, &points) {
                flattened = Some(candidate);
            }
        }
        match flattened {
            Some(flat) => {
                let mut corners = Vec::new();
                for &face in &chart {
                    let start = faces.starts[face as usize];
                    for (k, p) in faces.polygons[face as usize].iter().enumerate() {
                        let local = flat.local.get(p).copied().unwrap_or(0);
                        corners.push((start + k as u32, flat.uv[local as usize]));
                    }
                }
                islands.push(Island {
                    corners,
                    faces: chart,
                });
            }
            None => {
                let (left, right) = bisect(&faces, &chart);
                let all_join = |_: u32, _: u32| true;
                pending.extend(components(&faces, &topology, &right, all_join));
                pending.extend(components(&faces, &topology, &left, all_join));
            }
        }
    }
    pack_islands(geo, &islands, 0.004, 1, ctx)
}

/// Re-packs the islands of existing vertex or point `v@uv` into `tiles` UDIM tiles (row-major,
/// ten per row), with `padding` around each island. Islands are connected regions with continuous
/// UVs across mesh edges and keep their relative scale and shape.
pub fn uv_layout(
    geo: &GeoDetail,
    padding: f64,
    tiles: u32,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if !(padding.is_finite() && (0.0..0.25).contains(&padding)) || tiles == 0 || tiles > 100 {
        return Err(GeoError::InvalidTopology);
    }
    ctx.check_output(geo.vertex_count() * 4, geo.vertex_count() * 8)?;
    let faces = load_faces(geo)?;
    let topology = TopologyIndex::build(geo);
    let corner_uv = |he: u32| -> Result<[f64; 2], GeoError> {
        let read =
            |column: &AttributeColumn, index: usize| column.as_vec3().and_then(|c| c.get(index));
        let value = if let Some(column) = geo.domain_attribute(AttrDomain::Vertex, "uv") {
            read(column, he as usize)
        } else if let Some(column) = geo.domain_attribute(AttrDomain::Point, "uv") {
            let point = geo
                .vertex_point(he as usize)
                .ok_or(GeoError::IndexOutOfBounds)?;
            read(column, point as usize)
        } else {
            return Err(GeoError::AttributeNotFound);
        };
        let v = value.ok_or(GeoError::AttributeTypeMismatch)?;
        Ok([f64::from(v.x), f64::from(v.y)])
    };
    let mut uvs = Vec::with_capacity(geo.vertex_count());
    for he in 0..geo.vertex_count() as u32 {
        ctx.poll_every(he as usize)?;
        uvs.push(corner_uv(he)?);
    }
    let mut continuous = BTreeSet::new();
    for he in 0..geo.vertex_count() as u32 {
        let (Some(twin), Some(next), Some(twin_next)) = (
            topology.twin(he),
            topology.next(he),
            topology.twin(he).and_then(|t| topology.next(t)),
        ) else {
            continue;
        };
        if uvs[he as usize] == uvs[twin_next as usize] && uvs[next as usize] == uvs[twin as usize] {
            let pair = (faces.he_face[he as usize], faces.he_face[twin as usize]);
            continuous.insert((pair.0.min(pair.1), pair.0.max(pair.1)));
        }
    }
    let all: Vec<u32> = (0..faces.polygons.len() as u32).collect();
    let groups = components(&faces, &topology, &all, |a, b| {
        continuous.contains(&(a.min(b), a.max(b)))
    });
    let mut islands = Vec::with_capacity(groups.len());
    for group in groups {
        let mut corners = Vec::new();
        for &face in &group {
            let start = faces.starts[face as usize];
            for k in 0..faces.polygons[face as usize].len() as u32 {
                corners.push((start + k, uvs[(start + k) as usize]));
            }
        }
        islands.push(Island {
            corners,
            faces: group,
        });
    }
    pack_islands(geo, &islands, padding, tiles, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{create_sphere, create_torus, create_tube};
    use pcc_geo::{PrimKind, PrimRun};
    use std::f64::consts::PI;
    use std::sync::atomic::AtomicBool;

    fn vertex_uvs(geo: &GeoDetail) -> Result<Vec<[f64; 2]>, GeoError> {
        let column = geo
            .domain_attribute(AttrDomain::Vertex, "uv")
            .and_then(AttributeColumn::as_vec3)
            .ok_or(GeoError::AttributeNotFound)?;
        (0..column.len())
            .map(|i| {
                let v = column.get(i).ok_or(GeoError::IndexOutOfBounds)?;
                Ok([f64::from(v.x), f64::from(v.y)])
            })
            .collect()
    }

    /// Returns `(flipped triangle count, triangle count)` across all polygons.
    fn flip_count(geo: &GeoDetail) -> Result<(usize, usize), GeoError> {
        let uvs = vertex_uvs(geo)?;
        let (mut flips, mut total) = (0, 0);
        let mut cursor = 0u32;
        for polygon in load_polygons(geo)? {
            for k in 1..polygon.len() - 1 {
                let t = [cursor, cursor + k as u32, cursor + k as u32 + 1];
                total += 1;
                if signed_uv_area(&uvs, &t) <= 0.0 {
                    flips += 1;
                }
            }
            cursor += polygon.len() as u32;
        }
        Ok((flips, total))
    }

    fn assert_in_unit_square(geo: &GeoDetail) -> Result<(), GeoError> {
        for uv in vertex_uvs(geo)? {
            assert!(
                (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]),
                "{uv:?}"
            );
        }
        Ok(())
    }

    /// UV-space axis-aligned box as `(minimum, maximum)`.
    type UvBox = ([f64; 2], [f64; 2]);

    fn island_boxes(geo: &GeoDetail) -> Result<BTreeMap<i32, UvBox>, GeoError> {
        let tags = geo
            .domain_attribute(AttrDomain::Prim, "uv_island")
            .and_then(AttributeColumn::as_i32)
            .ok_or(GeoError::AttributeNotFound)?;
        let uvs = vertex_uvs(geo)?;
        let mut boxes: BTreeMap<i32, UvBox> = BTreeMap::new();
        let mut cursor = 0usize;
        for (prim, polygon) in load_polygons(geo)?.iter().enumerate() {
            let tag = tags.get(prim).ok_or(GeoError::IndexOutOfBounds)?;
            let entry = boxes
                .entry(tag)
                .or_insert(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]));
            for corner in &uvs[cursor..cursor + polygon.len()] {
                for (axis, &value) in corner.iter().enumerate() {
                    entry.0[axis] = entry.0[axis].min(value);
                    entry.1[axis] = entry.1[axis].max(value);
                }
            }
            cursor += polygon.len();
        }
        Ok(boxes)
    }

    fn assert_boxes_disjoint(geo: &GeoDetail) -> Result<(), GeoError> {
        let boxes: Vec<_> = island_boxes(geo)?.into_values().collect();
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let separated = a.1[0] <= b.0[0] + 1e-6
                    || b.1[0] <= a.0[0] + 1e-6
                    || a.1[1] <= b.0[1] + 1e-6
                    || b.1[1] <= a.0[1] + 1e-6;
                assert!(separated, "{a:?} overlaps {b:?}");
            }
        }
        Ok(())
    }

    fn flat_grid(n: u32, wrap_radius: Option<f64>) -> Result<GeoDetail, GeoError> {
        let mut points = Vec::new();
        for i in 0..n {
            for j in 0..n {
                let (s, t) = (
                    f64::from(j) / f64::from(n - 1),
                    f64::from(i) / f64::from(n - 1),
                );
                let p = match wrap_radius {
                    None => [4.0 * s, 0.0, 2.0 * t],
                    Some(r) => {
                        let angle = PI * s;
                        [r * angle.cos(), r * angle.sin(), 2.0 * t]
                    }
                };
                points.push(Vec3::checked(p[0] as f32, p[1] as f32, p[2] as f32)?);
            }
        }
        let mut vertices = Vec::new();
        for i in 0..n - 1 {
            for j in 0..n - 1 {
                let a = i * n + j;
                vertices.extend_from_slice(&[a, a + n, a + n + 1, a + 1]);
            }
        }
        GeoDetail::points(points).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 4,
                prim_count: ((n - 1) * (n - 1)) as usize,
            }],
            vertices,
        )
    }

    /// Spread of UV length / surface length across all edges: 1 means a perfect similarity.
    fn edge_scale_spread(geo: &GeoDetail) -> Result<f64, GeoError> {
        let points = load_points(geo)?;
        let uvs = vertex_uvs(geo)?;
        let (mut low, mut high) = (f64::INFINITY, 0.0f64);
        let mut cursor = 0usize;
        for polygon in load_polygons(geo)? {
            for k in 0..polygon.len() {
                let next = (k + 1) % polygon.len();
                let surface = distance(points[polygon[k] as usize], points[polygon[next] as usize]);
                let (a, b) = (uvs[cursor + k], uvs[cursor + next]);
                let flat = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
                let ratio = flat / surface;
                low = low.min(ratio);
                high = high.max(ratio);
            }
            cursor += polygon.len();
        }
        Ok(high / low)
    }

    #[test]
    fn a_flat_patch_flattens_isometrically_into_one_island() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 26, &flag);
        let out = uv_flatten(&flat_grid(9, None)?, 60.0, &ctx)?;
        assert_eq!(island_boxes(&out)?.len(), 1);
        assert_eq!(flip_count(&out)?.0, 0);
        let spread = edge_scale_spread(&out)?;
        assert!(spread < 1.0005, "spread {spread}");
        assert_in_unit_square(&out)
    }

    #[test]
    fn a_developable_half_cylinder_unrolls_with_little_distortion() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 26, &flag);
        let out = uv_flatten(&flat_grid(33, Some(1.0))?, 90.0, &ctx)?;
        assert_eq!(island_boxes(&out)?.len(), 1);
        assert_eq!(flip_count(&out)?.0, 0);
        let spread = edge_scale_spread(&out)?;
        assert!(spread < 1.02, "spread {spread}");
        Ok(())
    }

    #[test]
    fn closed_surfaces_are_cut_into_disks_with_zero_flips() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 28, &flag);
        for (geo, seam, minimum_islands) in [
            (create_sphere(1.0, 16, 24, &ctx)?, 180.0, 2),
            (create_torus(2.0, 0.6, 32, 16, &ctx)?, 180.0, 2),
            (create_tube(1.0, 2.0, 2, 24, &ctx)?, 45.0, 3),
        ] {
            let out = uv_flatten(&geo, seam, &ctx)?;
            let (flips, total) = flip_count(&out)?;
            assert_eq!(flips, 0, "{flips} of {total} triangles inverted");
            assert!(island_boxes(&out)?.len() >= minimum_islands);
            assert_in_unit_square(&out)?;
            assert_boxes_disjoint(&out)?;
        }
        Ok(())
    }

    #[test]
    fn hard_edges_split_a_box_into_one_island_per_face() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 24, &flag);
        let cube = create_tube(1.0, 1.0, 1, 4, &ctx)?;
        let out = uv_flatten(&cube, 30.0, &ctx)?;
        assert_eq!(island_boxes(&out)?.len(), 6);
        assert_boxes_disjoint(&out)?;
        assert_eq!(flip_count(&out)?.0, 0);
        Ok(())
    }

    #[test]
    fn layout_preserves_density_and_separates_tiles() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 26, &flag);
        let flattened = uv_flatten(&create_sphere(1.0, 12, 16, &ctx)?, 180.0, &ctx)?;
        let single = uv_layout(&flattened, 0.01, 1, &ctx)?;
        assert_in_unit_square(&single)?;
        assert_boxes_disjoint(&single)?;
        assert_eq!(flip_count(&single)?.0, 0);
        assert_eq!(
            island_boxes(&single)?.len(),
            island_boxes(&flattened)?.len()
        );

        let spread_before = edge_scale_spread(&flattened)?;
        let spread_after = edge_scale_spread(&single)?;
        assert!((spread_before - spread_after).abs() < 0.05 * spread_before);

        // Four tiles fit each island larger than one tile does, and stay inside their tile.
        let udim = uv_layout(&flattened, 0.01, 4, &ctx)?;
        let tiled = island_boxes(&udim)?;
        let packed = island_boxes(&single)?;
        let width = |entry: &UvBox| entry.1[0] - entry.0[0];
        let (first_tiled, first_packed) = (
            tiled.values().next().map(width).unwrap_or(0.0),
            packed.values().next().map(width).unwrap_or(1.0),
        );
        assert!(first_tiled > first_packed);
        for (low, high) in tiled.values() {
            assert_eq!(
                low[0].floor(),
                (high[0] - 1e-6).floor(),
                "{low:?}..{high:?}"
            );
            assert_eq!(
                low[1].floor(),
                (high[1] - 1e-6).floor(),
                "{low:?}..{high:?}"
            );
        }
        assert_boxes_disjoint(&udim)?;
        Ok(())
    }

    #[test]
    fn layout_validates_inputs_and_flatten_is_deterministic() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 26, &flag);
        let grid = flat_grid(4, None)?;
        assert_eq!(
            uv_layout(&grid, 0.01, 1, &ctx).err(),
            Some(GeoError::AttributeNotFound)
        );
        let flat = uv_flatten(&grid, 60.0, &ctx)?;
        assert!(uv_layout(&flat, 0.5, 1, &ctx).is_err());
        assert!(uv_layout(&flat, 0.01, 0, &ctx).is_err());
        assert_eq!(uv_flatten(&grid, 60.0, &ctx)?.hash(), flat.hash());
        Ok(())
    }
    #[test]
    fn single_faces_flatten_with_positive_area_at_any_shape_or_orientation() {
        // Slivers and arbitrary orientations are what broke the general path on real models.
        let cases: [[D3; 3]; 4] = [
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
            [[5.0, -2.0, 1.0], [5.0, 8.0, 1.0], [5.0, 3.0, 1.00001]],
            [[1.0, 1.0, 1.0], [2.0, 2.0, 2.0], [3.0, 3.0, 3.000001]],
        ];
        for points in cases {
            let uv = flatten_single_face(&points, &[0, 1, 2]);
            let flat_area = signed_uv_area(&uv, &[0, 1, 2]);
            let true_area = length(polygon_area_vector(&points, &[0, 1, 2])) * 0.5;
            assert!(flat_area > 0.0, "{points:?}");
            assert!(
                (flat_area - true_area).abs() < 1e-9,
                "{flat_area} vs {true_area}"
            );
        }
        // Zero-area faces still get a valid counter-clockwise placeholder.
        let collapsed = [[1.0, 1.0, 1.0]; 3];
        assert!(signed_uv_area(&flatten_single_face(&collapsed, &[0, 1, 2]), &[0, 1, 2]) > 0.0);
    }

    /// `(mesh area, uv area)` of every polygon, grouped by island.
    fn island_areas(geo: &GeoDetail) -> Result<BTreeMap<i32, Vec<(f64, f64)>>, GeoError> {
        let tags = geo
            .domain_attribute(AttrDomain::Prim, "uv_island")
            .and_then(AttributeColumn::as_i32)
            .ok_or(GeoError::AttributeNotFound)?;
        let points = load_points(geo)?;
        let uvs = vertex_uvs(geo)?;
        let mut grouped: BTreeMap<i32, Vec<(f64, f64)>> = BTreeMap::new();
        let mut cursor = 0u32;
        for (prim, polygon) in load_polygons(geo)?.iter().enumerate() {
            let mesh_area = length(polygon_area_vector(&points, polygon)) * 0.5;
            let mut uv_area = 0.0;
            for k in 1..polygon.len() - 1 {
                let k = k as u32;
                uv_area += signed_uv_area(&uvs, &[cursor, cursor + k, cursor + k + 1]);
            }
            let tag = tags.get(prim).ok_or(GeoError::IndexOutOfBounds)?;
            grouped.entry(tag).or_default().push((mesh_area, uv_area));
            cursor += polygon.len() as u32;
        }
        Ok(grouped)
    }

    #[test]
    fn chart_acceptance_rejects_inversion_and_excess_distortion_then_normalizes_density()
    -> Result<(), GeoError> {
        // Two triangles of a unit square, sharing diagonal (0, 2). Each has mesh area 1/2.
        let square = GeoDetail::points(vec![
            Vec3::checked(0.0, 0.0, 0.0)?,
            Vec3::checked(1.0, 0.0, 0.0)?,
            Vec3::checked(1.0, 1.0, 0.0)?,
            Vec3::checked(0.0, 1.0, 0.0)?,
        ])
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 2,
            }],
            vec![0, 1, 2, 0, 2, 3],
        )?;
        let faces = load_faces(&square)?;
        let points = load_points(&square)?;
        // The last corner sets the second triangle's UV area; the first always has area 1/2.
        let try_corner = |corner: [f64; 2]| {
            let mut flat = Flattened {
                local: (0..4u32).map(|p| (p, p)).collect(),
                uv: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], corner],
            };
            let accepted = accept_chart(&mut flat, &faces, &[0, 1], &points);
            (accepted, flat)
        };
        // Area 0.01: a magnification ratio of 50 is inside the limit of 64.
        let (accepted, flat) = try_corner([0.49, 0.51]);
        assert!(accepted);
        // Accepted charts are scaled so that total UV area equals total mesh area.
        let scaled = signed_uv_area(&flat.uv, &[0, 1, 2]) + signed_uv_area(&flat.uv, &[0, 2, 3]);
        assert!((scaled - 1.0).abs() < 1e-12, "{scaled}");
        // Area 0.005: a ratio of 100 exceeds the limit.
        assert!(!try_corner([0.495, 0.505]).0);
        // The second triangle turned over, and collapsed to zero area.
        assert!(!try_corner([0.6, 0.4]).0);
        assert!(!try_corner([0.5, 0.5]).0);
        Ok(())
    }

    /// The faces of a unit sphere whose centroid height lies within `[low, high)`.
    fn sphere_region(low: f64, high: f64, ctx: &SopContext<'_>) -> Result<GeoDetail, GeoError> {
        let sphere = create_sphere(1.0, 24, 32, ctx)?;
        let points = load_points(&sphere)?;
        let prims: Vec<_> = load_polygons(&sphere)?
            .into_iter()
            .filter(|polygon| {
                let total: f64 = polygon.iter().map(|&p| points[p as usize][1]).sum();
                (low..high).contains(&(total / polygon.len() as f64))
            })
            .map(|polygon| (PrimKind::Polygon, polygon))
            .collect();
        crate::common::build_geo(&points, &prims, ctx)
    }

    #[test]
    fn all_islands_share_one_texel_density() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 26, &flag);
        // A conformally flattened dome and a flat patch have different natural scales, so equal
        // densities can only come from the explicit per-chart normalization.
        let dome = sphere_region(0.0, f64::INFINITY, &ctx)?;
        let both = dome.merged(&flat_grid(5, None)?, ctx.memory_limit)?;
        let out = uv_flatten(&both, 180.0, &ctx)?;
        let densities: Vec<f64> = island_areas(&out)?
            .values()
            .map(|faces| {
                let mesh: f64 = faces.iter().map(|f| f.0).sum();
                let uv: f64 = faces.iter().map(|f| f.1).sum();
                uv / mesh
            })
            .collect();
        assert!(densities.len() >= 2);
        let low = densities.iter().copied().fold(f64::INFINITY, f64::min);
        let high = densities.iter().copied().fold(0.0f64, f64::max);
        assert!(high / low < 1.001, "density spread {low}..{high}");
        Ok(())
    }

    #[test]
    fn near_closed_surfaces_are_split_to_bound_area_distortion() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 26, &flag);
        // A sphere without its polar cap is one strongly curved disk chart.
        let punctured = sphere_region(f64::NEG_INFINITY, 30f64.to_radians().cos(), &ctx)?;
        let out = uv_flatten(&punctured, 180.0, &ctx)?;
        assert_eq!(flip_count(&out)?.0, 0);
        for faces in island_areas(&out)?.values() {
            let magnification = |&(mesh, uv): &(f64, f64)| uv / mesh;
            let low = faces
                .iter()
                .map(magnification)
                .fold(f64::INFINITY, f64::min);
            let high = faces.iter().map(magnification).fold(0.0f64, f64::max);
            assert!(high <= MAX_AREA_DISTORTION * low * 1.001, "{low}..{high}");
        }
        Ok(())
    }

    #[test]
    fn layout_padding_separates_islands_and_clears_tile_edges() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 26, &flag);
        let padding = 0.02;
        let flattened = uv_flatten(&create_tube(1.0, 2.0, 2, 24, &ctx)?, 45.0, &ctx)?;
        let packed = uv_layout(&flattened, padding, 1, &ctx)?;
        let boxes: Vec<UvBox> = island_boxes(&packed)?.into_values().collect();
        assert!(boxes.len() >= 3);
        for (i, a) in boxes.iter().enumerate() {
            for axis in 0..2 {
                assert!(
                    a.0[axis] >= padding * 0.5 - 1e-5,
                    "{a:?} touches the tile edge"
                );
                assert!(
                    a.1[axis] <= 1.0 - padding * 0.5 + 1e-5,
                    "{a:?} touches the tile edge"
                );
            }
            for b in &boxes[i + 1..] {
                let gap = (b.0[0] - a.1[0])
                    .max(a.0[0] - b.1[0])
                    .max(b.0[1] - a.1[1])
                    .max(a.0[1] - b.1[1]);
                assert!(gap >= padding - 1e-5, "gap {gap} between {a:?} and {b:?}");
            }
        }
        Ok(())
    }
}
