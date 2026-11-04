//! Half-space clipping with watertight capping, and Voronoi fracture built on top of it.

use crate::common::{
    D3, OrdF64, SopContext, any_perpendicular, build_geo, cross, distance, dot, has_repeated_faces,
    lerp, load_points, normalized, scale, sub,
};
use crate::earclip::{P2, bridge_holes, contains_point, ear_clip, signed_area};
use crate::modify::triangles_of;
use pcc_geo::{AttrDomain, AttributeColumn, GeoDetail, GeoError, PrimKind, TopologyIndex};
use std::collections::BTreeMap;

#[derive(Clone, Default)]
struct Soup {
    points: Vec<D3>,
    triangles: Vec<[u32; 3]>,
}

/// Unit plane normal `n` through `origin`; the kept side is `dot(p - origin, n) <= 0`.
struct Plane {
    origin: D3,
    normal: D3,
    tolerance: f64,
}

fn extent_of(points: &[D3]) -> f64 {
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for p in points {
        for (axis, &value) in p.iter().enumerate() {
            low[axis] = low[axis].min(value);
            high[axis] = high[axis].max(value);
        }
    }
    if low[0].is_finite() {
        distance(low, high)
    } else {
        0.0
    }
}

/// Clips `soup` by `plane`. With `cap`, openings created on the plane are closed by planar
/// polygons, keeping closed solids closed. Vertices are snapped to the plane within tolerance so
/// that near-coplanar geometry cannot produce slivers or cracks.
fn clip_soup(soup: &Soup, plane: &Plane, cap: bool) -> Soup {
    let side: Vec<f64> = soup
        .points
        .iter()
        .map(|&p| {
            let d = dot(sub(p, plane.origin), plane.normal);
            if d.abs() <= plane.tolerance { 0.0 } else { d }
        })
        .collect();
    let mut points = soup.points.clone();
    let mut on_plane: Vec<bool> = side.iter().map(|&d| d == 0.0).collect();
    let mut splits: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    let mut triangles = Vec::with_capacity(soup.triangles.len());
    for t in &soup.triangles {
        let d = [
            side[t[0] as usize],
            side[t[1] as usize],
            side[t[2] as usize],
        ];
        let any_inside = d.iter().any(|&v| v < 0.0);
        let any_outside = d.iter().any(|&v| v > 0.0);
        if !any_inside && !any_outside {
            // Coplanar: this face already closes the solid when it looks out of the kept side.
            let area_vector = cross(
                sub(soup.points[t[1] as usize], soup.points[t[0] as usize]),
                sub(soup.points[t[2] as usize], soup.points[t[0] as usize]),
            );
            if dot(area_vector, plane.normal) > 0.0 {
                triangles.push(*t);
            }
        } else if !any_outside {
            triangles.push(*t);
        } else if any_inside {
            let mut kept: Vec<u32> = Vec::with_capacity(4);
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let (da, db) = (d[k], d[(k + 1) % 3]);
                if da <= 0.0 {
                    kept.push(a);
                }
                if (da < 0.0 && db > 0.0) || (da > 0.0 && db < 0.0) {
                    let key = (a.min(b), a.max(b));
                    let split = *splits.entry(key).or_insert_with(|| {
                        // Interpolate in canonical direction so both faces agree bit-for-bit.
                        let (low, high) = (side[key.0 as usize], side[key.1 as usize]);
                        let t = low / (low - high);
                        points.push(lerp(
                            soup.points[key.0 as usize],
                            soup.points[key.1 as usize],
                            t,
                        ));
                        on_plane.push(true);
                        (points.len() - 1) as u32
                    });
                    kept.push(split);
                }
            }
            for k in 1..kept.len().saturating_sub(1) {
                triangles.push([kept[0], kept[k], kept[k + 1]]);
            }
        }
    }
    if cap {
        cap_openings(&points, &on_plane, plane.normal, &mut triangles);
    }
    compact(&points, &triangles)
}

fn compact(points: &[D3], triangles: &[[u32; 3]]) -> Soup {
    let mut remap = vec![u32::MAX; points.len()];
    let mut out = Soup::default();
    for t in triangles {
        if t[0] == t[1] || t[1] == t[2] || t[2] == t[0] {
            continue;
        }
        let mut mapped = [0u32; 3];
        for (slot, &v) in mapped.iter_mut().zip(t) {
            if remap[v as usize] == u32::MAX {
                remap[v as usize] = out.points.len() as u32;
                out.points.push(points[v as usize]);
            }
            *slot = remap[v as usize];
        }
        out.triangles.push(mapped);
    }
    out
}

/// Chains unmatched on-plane directed edges into simple cycles.
fn extract_loops(edges: &[(u32, u32)]) -> Vec<Vec<u32>> {
    let mut outgoing: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &(a, b) in edges {
        outgoing.entry(a).or_default().push(b);
    }
    let mut loops = Vec::new();
    let starts: Vec<u32> = outgoing.keys().copied().collect();
    for start in starts {
        while outgoing
            .get(&start)
            .is_some_and(|targets| !targets.is_empty())
        {
            let mut path = vec![start];
            let mut current = start;
            while let Some(next) = outgoing.get_mut(&current).and_then(Vec::pop) {
                if let Some(position) = path.iter().position(|&v| v == next) {
                    // Closing onto the path peels off one simple cycle, even at pinch vertices.
                    loops.push(path.split_off(position));
                    path.push(next);
                }
                if path.last() != Some(&next) {
                    path.push(next);
                }
                current = next;
                if path.len() == 1 && next == start {
                    break;
                }
            }
        }
    }
    loops
}

fn cap_openings(points: &[D3], on_plane: &[bool], normal: D3, triangles: &mut Vec<[u32; 3]>) {
    let mut directed: Vec<(u32, u32)> = Vec::with_capacity(triangles.len() * 3);
    for t in triangles.iter() {
        if t[0] == t[1] || t[1] == t[2] || t[2] == t[0] {
            continue;
        }
        directed.extend_from_slice(&[(t[0], t[1]), (t[1], t[2]), (t[2], t[0])]);
    }
    directed.sort_unstable();
    let mut openings: Vec<(u32, u32)> = Vec::new();
    for &(a, b) in &directed {
        if on_plane[a as usize] && on_plane[b as usize] && directed.binary_search(&(b, a)).is_err()
        {
            // The cap traverses the opening opposite to the surface it closes.
            openings.push((b, a));
        }
    }
    if openings.is_empty() {
        return;
    }
    let u = any_perpendicular(normal);
    let v = cross(normal, u);
    let mut flat: Vec<P2> = Vec::with_capacity(points.len());
    for &p in points {
        flat.push([dot(p, u), dot(p, v)]);
    }
    let as_corners = |l: &[u32]| l.iter().map(|&i| i as usize).collect::<Vec<_>>();
    let mut outers: Vec<(Vec<usize>, f64, Vec<Vec<usize>>)> = Vec::new();
    let mut holes = Vec::new();
    for found in extract_loops(&openings) {
        if found.len() < 3 {
            continue;
        }
        let corners = as_corners(&found);
        let area = signed_area(&flat, &corners);
        if area >= 0.0 {
            outers.push((corners, area, Vec::new()));
        } else {
            holes.push(corners);
        }
    }
    // Every opening must be consumed or the solid stays open. A clockwise loop inside no outline is
    // not a hole: inside-out solids produce only such loops. It is capped as its own polygon.
    let mut orphans = Vec::new();
    for hole in holes {
        let probe = flat[hole[0]];
        let home = outers
            .iter()
            .enumerate()
            .filter(|(_, outer)| contains_point(&flat, &outer.0, probe))
            .min_by_key(|(_, outer)| OrdF64(outer.1))
            .map(|(index, _)| index);
        match home {
            Some(index) => outers[index].2.push(hole),
            None => orphans.push(hole),
        }
    }
    for orphan in orphans {
        let reversed: Vec<usize> = orphan.iter().rev().copied().collect();
        for t in ear_clip(&flat, &reversed) {
            // Triangulated counter-clockwise; restored to the loop's own direction.
            triangles.push([t[2] as u32, t[1] as u32, t[0] as u32]);
        }
    }
    for (outer, _, inner) in outers {
        let merged = bridge_holes(&flat, outer, inner);
        for t in ear_clip(&flat, &merged) {
            triangles.push([t[0] as u32, t[1] as u32, t[2] as u32]);
        }
    }
}

fn soup_to_geo(soup: &Soup, ctx: &SopContext<'_>) -> Result<GeoDetail, GeoError> {
    let prims: Vec<_> = soup
        .triangles
        .iter()
        .map(|t| (PrimKind::Polygon, t.to_vec()))
        .collect();
    build_geo(&soup.points, &prims, ctx)
}

/// Removes everything on the +`normal` side of the plane through `origin`. With `cap`, closed
/// inputs stay closed. Output is triangles and carries only positions.
pub fn clip_halfspace(
    geo: &GeoDetail,
    origin: [f64; 3],
    normal: [f64; 3],
    cap: bool,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    let unit = normalized(normal).ok_or(GeoError::InvalidTopology)?;
    ctx.poll()?;
    let (points, triangles) = triangles_of(geo)?;
    let tolerance = extent_of(&points) * 1e-7;
    let soup = Soup { points, triangles };
    let plane = Plane {
        origin,
        normal: unit,
        tolerance,
    };
    soup_to_geo(&clip_soup(&soup, &plane, cap), ctx)
}

/// Cuts a closed manifold into the Voronoi cells of the `seeds` points, each piece being its own
/// closed shell tagged with `i@piece` (the seed index) on points and primitives. Seeds that
/// duplicate an earlier one and cells that miss the solid produce no piece. Output is triangles.
pub fn voronoi_fracture(
    geo: &GeoDetail,
    seeds: &GeoDetail,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if !TopologyIndex::build(geo).is_closed_manifold() || has_repeated_faces(geo) {
        return Err(GeoError::InvalidTopology);
    }
    let (points, triangles) = triangles_of(geo)?;
    let extent = extent_of(&points);
    let tolerance = extent * 1e-7;
    let seed_points = load_points(seeds)?;
    if seed_points.is_empty() {
        return Err(GeoError::InvalidTopology);
    }
    let base = Soup { points, triangles };
    let mut out_points: Vec<D3> = Vec::new();
    let mut out_prims: Vec<(PrimKind, Vec<u32>)> = Vec::new();
    let mut point_piece: Vec<i32> = Vec::new();
    let mut prim_piece: Vec<i32> = Vec::new();
    for (seed_index, &seed) in seed_points.iter().enumerate() {
        ctx.poll()?;
        if seed_points[..seed_index]
            .iter()
            .any(|&earlier| distance(earlier, seed) <= tolerance)
        {
            continue;
        }
        let mut others: Vec<(OrdF64, usize)> = seed_points
            .iter()
            .enumerate()
            .filter(|&(j, &other)| j != seed_index && distance(other, seed) > tolerance)
            .map(|(j, &other)| (OrdF64(distance(other, seed)), j))
            .collect();
        others.sort_unstable();
        let mut piece = base.clone();
        for (gap, j) in others {
            ctx.poll()?;
            // Once the bisector is beyond the farthest vertex, no later seed can cut this cell.
            let reach = piece
                .points
                .iter()
                .map(|&p| distance(p, seed))
                .fold(0.0f64, f64::max);
            if gap.0 * 0.5 > reach {
                break;
            }
            let other = seed_points[j];
            let plane = Plane {
                origin: lerp(seed, other, 0.5),
                normal: scale(sub(other, seed), 1.0 / gap.0),
                tolerance,
            };
            piece = clip_soup(&piece, &plane, true);
            if piece.triangles.is_empty() {
                break;
            }
        }
        if piece.triangles.is_empty() {
            continue;
        }
        let offset = u32::try_from(out_points.len()).map_err(|_| GeoError::LimitExceeded)?;
        let tag = i32::try_from(seed_index).map_err(|_| GeoError::LimitExceeded)?;
        ctx.check_output(
            out_points.len() + piece.points.len(),
            (out_prims.len() + piece.triangles.len()) * 3,
        )?;
        out_points.extend_from_slice(&piece.points);
        point_piece.extend(std::iter::repeat_n(tag, piece.points.len()));
        for t in &piece.triangles {
            out_prims.push((
                PrimKind::Polygon,
                vec![t[0] + offset, t[1] + offset, t[2] + offset],
            ));
            prim_piece.push(tag);
        }
    }
    build_geo(&out_points, &out_prims, ctx)?
        .with_domain_attribute(
            AttrDomain::Point,
            "piece",
            AttributeColumn::i32_values(point_piece),
        )?
        .with_domain_attribute(
            AttrDomain::Prim,
            "piece",
            AttributeColumn::i32_values(prim_piece),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{load_polygons, signed_volume};
    use crate::primitives::{create_sphere, create_torus};
    use pcc_geo::PrimRun;
    use pcc_math::{Philox4x32, Vec3};
    use std::f64::consts::PI;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn cube() -> Result<GeoDetail, GeoError> {
        let v = |x: f32, y: f32, z: f32| Vec3::checked(x, y, z);
        GeoDetail::points(vec![
            v(0., 0., 0.)?,
            v(1., 0., 0.)?,
            v(1., 1., 0.)?,
            v(0., 1., 0.)?,
            v(0., 0., 1.)?,
            v(1., 0., 1.)?,
            v(1., 1., 1.)?,
            v(0., 1., 1.)?,
        ])
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 4,
                prim_count: 6,
            }],
            vec![
                0, 3, 2, 1, 4, 5, 6, 7, 0, 1, 5, 4, 2, 3, 7, 6, 0, 4, 7, 3, 1, 2, 6, 5,
            ],
        )
    }

    fn random_seeds(count: u32, span: f32, seed: u64) -> Result<GeoDetail, GeoError> {
        let rng = Philox4x32::new(seed);
        let mut points = Vec::new();
        for i in 0..count {
            let u = rng.generate_unit_f32([i, 0, 0, 0]);
            points.push(Vec3::checked(
                (u[0] - 0.5) * span,
                (u[1] - 0.5) * span,
                (u[2] - 0.5) * span,
            )?);
        }
        Ok(GeoDetail::points(points))
    }

    /// Splits fracture output by `i@piece` and returns each shell's volume.
    fn piece_volumes(geo: &GeoDetail) -> Result<BTreeMap<i32, f64>, GeoError> {
        let tags = geo
            .domain_attribute(AttrDomain::Prim, "piece")
            .and_then(AttributeColumn::as_i32)
            .ok_or(GeoError::AttributeNotFound)?;
        let points = load_points(geo)?;
        let mut volumes = BTreeMap::new();
        for (prim, polygon) in load_polygons(geo)?.iter().enumerate() {
            let tag = tags.get(prim).ok_or(GeoError::IndexOutOfBounds)?;
            let triple = dot(
                points[polygon[0] as usize],
                cross(points[polygon[1] as usize], points[polygon[2] as usize]),
            );
            *volumes.entry(tag).or_insert(0.0) += triple / 6.0;
        }
        Ok(volumes)
    }

    #[test]
    fn cube_clip_produces_the_exact_closed_slab() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 22, &flag);
        let slab = clip_halfspace(&cube()?, [0.0, 0.25, 0.0], [0.0, 1.0, 0.0], true, &ctx)?;
        assert!(TopologyIndex::build(&slab).is_closed_manifold());
        assert!((signed_volume(&slab) - 0.25).abs() < 1e-6);
        let open = clip_halfspace(&cube()?, [0.0, 0.25, 0.0], [0.0, 1.0, 0.0], false, &ctx)?;
        assert_eq!(TopologyIndex::build(&open).boundary_edge_count() % 4, 0);
        assert!(TopologyIndex::build(&open).boundary_edge_count() >= 4);
        Ok(())
    }

    #[test]
    fn clip_through_existing_vertices_and_faces_stays_exact() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 22, &flag);
        // Diagonal plane through four cube vertices: exactly half the volume remains.
        let half = clip_halfspace(&cube()?, [0.0, 0.0, 0.0], [1.0, -1.0, 0.0], true, &ctx)?;
        assert!(TopologyIndex::build(&half).is_closed_manifold());
        assert!((signed_volume(&half) - 0.5).abs() < 1e-6);
        // A plane containing a face from outside keeps the solid unchanged; from inside removes it.
        let whole = clip_halfspace(&cube()?, [0.0, 1.0, 0.0], [0.0, 1.0, 0.0], true, &ctx)?;
        assert!((signed_volume(&whole) - 1.0).abs() < 1e-6);
        assert!(TopologyIndex::build(&whole).is_closed_manifold());
        let gone = clip_halfspace(&cube()?, [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], true, &ctx)?;
        assert_eq!(gone.prim_count(), 0);
        assert!(clip_halfspace(&cube()?, [0.0; 3], [0.0; 3], true, &ctx).is_err());
        Ok(())
    }

    #[test]
    fn torus_cross_section_caps_two_rings_and_hole_caps_annuli() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 26, &flag);
        let torus = create_torus(3.0, 1.0, 128, 64, &ctx)?;
        let whole = signed_volume(&torus);
        // Cutting the torus across its symmetry plane caps an annulus with a real hole.
        let half = clip_halfspace(&torus, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], true, &ctx)?;
        assert!(TopologyIndex::build(&half).is_closed_manifold());
        assert!((signed_volume(&half) - whole / 2.0).abs() < 1e-3 * whole);
        // Cutting through the axis caps two separate disks.
        let half2 = clip_halfspace(&torus, [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], true, &ctx)?;
        assert!(TopologyIndex::build(&half2).is_closed_manifold());
        assert!((signed_volume(&half2) - whole / 2.0).abs() < 1e-3 * whole);
        Ok(())
    }

    #[test]
    fn fracture_of_a_convex_solid_conserves_volume_and_closes_every_shell() -> Result<(), GeoError>
    {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 28, &flag);
        let sphere = create_sphere(1.0, 24, 32, &ctx)?;
        let seeds = random_seeds(24, 1.4, 42)?;
        let pieces = voronoi_fracture(&sphere, &seeds, &ctx)?;
        assert!(TopologyIndex::build(&pieces).is_closed_manifold());
        let volumes = piece_volumes(&pieces)?;
        assert!(volumes.len() >= 20, "{} pieces", volumes.len());
        assert!(volumes.values().all(|&v| v > 0.0));
        let total: f64 = volumes.values().sum();
        let whole = signed_volume(&sphere);
        assert!((total - whole).abs() < 1e-4 * whole, "{total} vs {whole}");
        assert!(4.0 / 3.0 * PI > whole);
        // Voronoi property: every point of a cell is at least as close to its own seed.
        let seed_positions = load_points(&seeds)?;
        let tags = pieces
            .attribute("piece")
            .and_then(AttributeColumn::as_i32)
            .ok_or(GeoError::AttributeNotFound)?;
        for (i, p) in load_points(&pieces)?.into_iter().enumerate() {
            let own = tags.get(i).ok_or(GeoError::IndexOutOfBounds)? as usize;
            let mine = distance(p, seed_positions[own]);
            for &other in &seed_positions {
                assert!(mine <= distance(p, other) + 1e-5);
            }
        }
        Ok(())
    }

    #[test]
    fn fracture_of_a_non_convex_solid_with_a_hole_conserves_volume() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 28, &flag);
        let torus = create_torus(2.0, 0.7, 48, 24, &ctx)?;
        let seeds = random_seeds(16, 5.0, 9)?;
        let pieces = voronoi_fracture(&torus, &seeds, &ctx)?;
        let topology = TopologyIndex::build(&pieces);
        assert_eq!(topology.boundary_edge_count(), 0);
        assert_eq!(topology.non_manifold_edge_count(), 0);
        let total: f64 = piece_volumes(&pieces)?.values().sum();
        let whole = signed_volume(&torus);
        assert!((total - whole).abs() < 1e-4 * whole, "{total} vs {whole}");
        let repeat = voronoi_fracture(&torus, &seeds, &ctx)?;
        assert_eq!(repeat.hash(), pieces.hash());
        Ok(())
    }

    #[test]
    fn inside_out_solids_clip_and_fracture_closed_with_negative_volume() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 24, &flag);
        let inverted = cube()?.with_reversed_winding()?;
        let slab = clip_halfspace(&inverted, [0.0, 0.25, 0.0], [0.0, 1.0, 0.0], true, &ctx)?;
        assert!(TopologyIndex::build(&slab).is_closed_manifold());
        assert!(
            (signed_volume(&slab) + 0.25).abs() < 1e-6,
            "{}",
            signed_volume(&slab)
        );
        // A cut through an inverted torus leaves two loops at once, and an annulus the other way.
        let torus = create_torus(3.0, 1.0, 48, 24, &ctx)?.with_reversed_winding()?;
        let whole = signed_volume(&torus);
        for normal in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            let half = clip_halfspace(&torus, [0.0; 3], normal, true, &ctx)?;
            assert!(TopologyIndex::build(&half).is_closed_manifold());
            assert!((signed_volume(&half) - whole / 2.0).abs() < 1e-3 * whole.abs());
        }
        let seeds = random_seeds(8, 4.0, 3)?;
        let shards = voronoi_fracture(&torus, &seeds, &ctx)?;
        assert!(TopologyIndex::build(&shards).is_closed_manifold());
        assert!((signed_volume(&shards) - whole).abs() < 1e-3 * whole.abs());
        Ok(())
    }

    #[test]
    fn fracture_rejects_open_input_duplicates_seeds_and_observes_cancellation()
    -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 24, &flag);
        let open = clip_halfspace(&cube()?, [0.0, 0.5, 0.0], [0.0, 1.0, 0.0], false, &ctx)?;
        let one_seed = GeoDetail::points(vec![Vec3::ZERO]);
        assert_eq!(
            voronoi_fracture(&open, &one_seed, &ctx).err(),
            Some(GeoError::InvalidTopology)
        );
        assert!(voronoi_fracture(&cube()?, &GeoDetail::points(Vec::new()), &ctx).is_err());
        // Coincident seeds are one seed: the first owns the entire solid.
        let doubled = GeoDetail::points(vec![Vec3::ZERO, Vec3::ZERO]);
        let single = voronoi_fracture(&cube()?, &doubled, &ctx)?;
        assert_eq!(piece_volumes(&single)?.len(), 1);
        flag.store(true, Ordering::Release);
        assert_eq!(
            voronoi_fracture(&cube()?, &one_seed, &ctx).err(),
            Some(GeoError::Cancelled)
        );
        Ok(())
    }
}
