//! Editable manifold triangle mesh with the local operations used by decimation and remeshing.

use crate::common::{D3, SopContext, build_geo, cross, dot, length, sub};
use crate::modify::triangles_of;
use pcc_geo::{GeoDetail, GeoError, PrimKind};
use std::collections::BTreeSet;

pub(crate) struct TriMesh {
    pub verts: Vec<D3>,
    pub tris: Vec<[u32; 3]>,
    pub tri_alive: Vec<bool>,
    pub vert_alive: Vec<bool>,
    /// Incident live triangles per vertex.
    pub vert_tris: Vec<Vec<u32>>,
    pub live_tris: usize,
}

impl TriMesh {
    /// Triangulates polygon input and verifies it is an oriented manifold (every directed edge
    /// is unique). Degenerate triangles and unreferenced points are dropped.
    pub(crate) fn from_geo(geo: &GeoDetail, ctx: &SopContext<'_>) -> Result<Self, GeoError> {
        let (points, triangles) = triangles_of(geo)?;
        let mut mesh = Self {
            verts: points,
            tris: Vec::with_capacity(triangles.len()),
            tri_alive: Vec::with_capacity(triangles.len()),
            vert_alive: Vec::new(),
            vert_tris: Vec::new(),
            live_tris: 0,
        };
        mesh.vert_alive = vec![false; mesh.verts.len()];
        mesh.vert_tris = vec![Vec::new(); mesh.verts.len()];
        let mut directed = Vec::with_capacity(triangles.len() * 3);
        for (i, t) in triangles.iter().enumerate() {
            ctx.poll_every(i)?;
            if t[0] == t[1] || t[1] == t[2] || t[2] == t[0] {
                continue;
            }
            directed.extend_from_slice(&[(t[0], t[1]), (t[1], t[2]), (t[2], t[0])]);
            for &v in t {
                mesh.vert_alive[v as usize] = true;
            }
            mesh.add_triangle(*t);
        }
        directed.sort_unstable();
        if directed.windows(2).any(|w| w[0] == w[1]) {
            return Err(GeoError::InvalidTopology);
        }
        Ok(mesh)
    }

    fn add_triangle(&mut self, t: [u32; 3]) -> u32 {
        let index = self.tris.len() as u32;
        self.tris.push(t);
        self.tri_alive.push(true);
        for &v in &t {
            self.vert_tris[v as usize].push(index);
        }
        self.live_tris += 1;
        index
    }

    fn remove_triangle(&mut self, index: u32) {
        if !self.tri_alive[index as usize] {
            return;
        }
        self.tri_alive[index as usize] = false;
        self.live_tris -= 1;
        for &v in &self.tris[index as usize] {
            self.vert_tris[v as usize].retain(|&t| t != index);
        }
    }

    pub(crate) fn add_vertex(&mut self, p: D3) -> u32 {
        let index = self.verts.len() as u32;
        self.verts.push(p);
        self.vert_alive.push(true);
        self.vert_tris.push(Vec::new());
        index
    }

    /// Sorted, unique neighbours of `v`.
    pub(crate) fn neighbours(&self, v: u32) -> Vec<u32> {
        let mut result = Vec::with_capacity(self.vert_tris[v as usize].len() * 2);
        for &t in &self.vert_tris[v as usize] {
            for &other in &self.tris[t as usize] {
                if other != v {
                    result.push(other);
                }
            }
        }
        result.sort_unstable();
        result.dedup();
        result
    }

    /// Live triangles containing both `a` and `b`.
    pub(crate) fn edge_triangles(&self, a: u32, b: u32) -> Vec<u32> {
        self.vert_tris[a as usize]
            .iter()
            .copied()
            .filter(|&t| self.tris[t as usize].contains(&b))
            .collect()
    }

    pub(crate) fn is_boundary_edge(&self, a: u32, b: u32) -> bool {
        self.edge_triangles(a, b).len() == 1
    }

    pub(crate) fn is_boundary_vertex(&self, v: u32) -> bool {
        self.neighbours(v)
            .into_iter()
            .any(|n| self.is_boundary_edge(v, n))
    }

    /// Every live edge exactly once as `(low, high)`, in sorted order.
    pub(crate) fn edges(&self) -> Vec<(u32, u32)> {
        let mut result = Vec::with_capacity(self.live_tris * 3);
        for (index, t) in self.tris.iter().enumerate() {
            if self.tri_alive[index] {
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    result.push((a.min(b), a.max(b)));
                }
            }
        }
        result.sort_unstable();
        result.dedup();
        result
    }

    /// Triangle normal scaled by twice the area, with `moved` substituted at `moved_position`.
    fn area_vector_with(&self, t: [u32; 3], moved: &[u32], position: D3) -> D3 {
        let at = |v: u32| {
            if moved.contains(&v) {
                position
            } else {
                self.verts[v as usize]
            }
        };
        cross(sub(at(t[1]), at(t[0])), sub(at(t[2]), at(t[0])))
    }

    /// Topological validity of collapsing edge `(a, b)`, per the link condition of Dey et al.
    /// (`Lk(a) ∩ Lk(b) = Lk(ab)`), which guarantees that collapse preserves the manifold.
    pub(crate) fn collapse_keeps_manifold(&self, a: u32, b: u32) -> bool {
        let shared = self.edge_triangles(a, b);
        if shared.is_empty() || shared.len() > 2 {
            return false;
        }
        let (neighbours_a, neighbours_b) = (self.neighbours(a), self.neighbours(b));
        let common = neighbours_a
            .iter()
            .filter(|n| neighbours_b.binary_search(n).is_ok())
            .count();
        if common != shared.len() {
            return false;
        }
        if shared.len() == 2 && self.is_boundary_vertex(a) && self.is_boundary_vertex(b) {
            return false;
        }
        // Without this a tetrahedron-like fold could collapse into two back-to-back triangles.
        let link_edges = |v: u32| {
            let mut edges = BTreeSet::new();
            for &t in &self.vert_tris[v as usize] {
                let others: Vec<u32> = self.tris[t as usize]
                    .iter()
                    .copied()
                    .filter(|&x| x != v)
                    .collect();
                if let [x, y] = others[..] {
                    edges.insert((x.min(y), x.max(y)));
                }
            }
            edges
        };
        link_edges(a).is_disjoint(&link_edges(b))
    }

    /// Geometric validity: no triangle around the collapse inverts or degenerates when `a` and
    /// `b` both move to `position`.
    pub(crate) fn collapse_keeps_orientation(&self, a: u32, b: u32, position: D3) -> bool {
        for v in [a, b] {
            for &t in &self.vert_tris[v as usize] {
                let tri = self.tris[t as usize];
                if tri.contains(&a) && tri.contains(&b) {
                    continue;
                }
                let before = self.area_vector_with(tri, &[], position);
                let after = self.area_vector_with(tri, &[a, b], position);
                let (len_before, len_after) = (length(before), length(after));
                if len_after <= 1e-14 * len_before.max(1e-300) || len_after == 0.0 {
                    return false;
                }
                if dot(before, after) < 0.2 * len_before * len_after {
                    return false;
                }
            }
        }
        true
    }

    /// Removes `drop`, merging it into `keep` placed at `position`. Callers verify validity first.
    pub(crate) fn collapse(&mut self, keep: u32, drop: u32, position: D3) {
        for t in self.edge_triangles(keep, drop) {
            self.remove_triangle(t);
        }
        let moved = std::mem::take(&mut self.vert_tris[drop as usize]);
        for t in moved {
            let tri = &mut self.tris[t as usize];
            for slot in tri.iter_mut() {
                if *slot == drop {
                    *slot = keep;
                }
            }
            self.vert_tris[keep as usize].push(t);
        }
        self.verts[keep as usize] = position;
        self.vert_alive[drop as usize] = false;
    }

    /// Splits edge `(a, b)` at `position`, returning the new vertex.
    pub(crate) fn split_edge(&mut self, a: u32, b: u32, position: D3) -> u32 {
        let middle = self.add_vertex(position);
        for t in self.edge_triangles(a, b) {
            let tri = self.tris[t as usize];
            self.remove_triangle(t);
            let (mut first, mut second) = (tri, tri);
            for k in 0..3 {
                if tri[k] == a {
                    first[k] = middle;
                }
                if tri[k] == b {
                    second[k] = middle;
                }
            }
            self.add_triangle(first);
            self.add_triangle(second);
        }
        middle
    }

    /// Replaces interior edge `(a, b)` by the opposite diagonal if that keeps the mesh valid and
    /// neither new triangle inverts. Returns whether the flip happened.
    pub(crate) fn flip_edge(&mut self, a: u32, b: u32) -> bool {
        let shared = self.edge_triangles(a, b);
        let [t0, t1] = shared[..] else {
            return false;
        };
        let opposite = |t: u32| {
            self.tris[t as usize]
                .iter()
                .copied()
                .find(|&v| v != a && v != b)
        };
        let (Some(c), Some(d)) = (opposite(t0), opposite(t1)) else {
            return false;
        };
        if c == d || !self.edge_triangles(c, d).is_empty() || self.neighbours(c).contains(&d) {
            return false;
        }
        // Orient so that t0 traverses a -> b, which t1 then traverses as b -> a.
        let tri0 = self.tris[t0 as usize];
        let forward = (0..3).any(|k| tri0[k] == a && tri0[(k + 1) % 3] == b);
        let (start, end) = if forward { (a, b) } else { (b, a) };
        let (top, bottom) = (c, d);
        let old0 = self.area_vector_with([start, end, top], &[], [0.0; 3]);
        let old1 = self.area_vector_with([end, start, bottom], &[], [0.0; 3]);
        let new0 = self.area_vector_with([start, bottom, top], &[], [0.0; 3]);
        let new1 = self.area_vector_with([end, top, bottom], &[], [0.0; 3]);
        let reference = crate::common::add(old0, old1);
        if dot(new0, reference) <= 0.0
            || dot(new1, reference) <= 0.0
            || length(new0) <= 1e-12 * length(reference)
            || length(new1) <= 1e-12 * length(reference)
        {
            return false;
        }
        self.remove_triangle(t0);
        self.remove_triangle(t1);
        self.add_triangle([start, bottom, top]);
        self.add_triangle([end, top, bottom]);
        true
    }

    /// Area-weighted vertex normal (not normalized).
    pub(crate) fn vertex_area_normal(&self, v: u32) -> D3 {
        let mut total = [0.0; 3];
        for &t in &self.vert_tris[v as usize] {
            let area = self.area_vector_with(self.tris[t as usize], &[], [0.0; 3]);
            total = crate::common::add(total, area);
        }
        total
    }

    /// Compacts live data into a triangle-mesh detail.
    pub(crate) fn to_geo(&self, ctx: &SopContext<'_>) -> Result<GeoDetail, GeoError> {
        let mut remap = vec![u32::MAX; self.verts.len()];
        let mut points = Vec::new();
        let mut prims = Vec::with_capacity(self.live_tris);
        for (index, t) in self.tris.iter().enumerate() {
            ctx.poll_every(index)?;
            if !self.tri_alive[index] {
                continue;
            }
            let mut mapped = Vec::with_capacity(3);
            for &v in t {
                if remap[v as usize] == u32::MAX {
                    remap[v as usize] = points.len() as u32;
                    points.push(self.verts[v as usize]);
                }
                mapped.push(remap[v as usize]);
            }
            prims.push((PrimKind::Polygon, mapped));
        }
        build_geo(&points, &prims, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcc_geo::PrimRun;
    use pcc_math::Vec3;
    use std::sync::atomic::AtomicBool;

    fn mesh(points: &[[f32; 3]], triangles: &[[u32; 3]]) -> Result<TriMesh, GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        let positions = points
            .iter()
            .map(|p| Vec3::checked(p[0], p[1], p[2]))
            .collect::<Result<Vec<_>, _>>()?;
        let geo = GeoDetail::points(positions).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: triangles.len(),
            }],
            triangles.iter().flatten().copied().collect(),
        )?;
        TriMesh::from_geo(&geo, &ctx)
    }

    #[test]
    fn link_condition_rejects_tetrahedron_collapse_and_extra_common_neighbours()
    -> Result<(), GeoError> {
        // Collapsing any tetrahedron edge would leave two back-to-back triangles.
        let tetrahedron = mesh(
            &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            &[[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]],
        )?;
        assert!(!tetrahedron.collapse_keeps_manifold(0, 1));

        // In a triangular bipyramid, ring edge (0, 1) has a third common neighbour, the far ring
        // vertex 2, beyond the two apexes on its faces: collapsing it pinches the surface.
        let bipyramid = mesh(
            &[
                [1., 0., 0.],
                [-0.5, 0., 0.866],
                [-0.5, 0., -0.866],
                [0., 1., 0.],
                [0., -1., 0.],
            ],
            &[
                [3, 1, 0],
                [3, 2, 1],
                [3, 0, 2],
                [4, 0, 1],
                [4, 1, 2],
                [4, 2, 0],
            ],
        )?;
        assert!(!bipyramid.collapse_keeps_manifold(0, 1));
        // Its apex edges are safe: they have exactly the two common neighbours on their faces.
        assert!(bipyramid.collapse_keeps_manifold(3, 0));
        Ok(())
    }

    #[test]
    fn link_condition_rejects_collapsing_across_a_non_facial_cycle() -> Result<(), GeoError> {
        // Three rings of three points forming an open triangular tube. The middle ring (3, 4, 5)
        // is a 3-cycle that bounds no face, so collapsing its edge (3, 4) would pinch the neck.
        // Their stars share no link edge: only the extra common neighbour 5 reveals the problem.
        let mut triangles = Vec::new();
        for ring in 0..2u32 {
            for j in 0..3u32 {
                let at = |r: u32, k: u32| r * 3 + k % 3;
                triangles.push([at(ring, j), at(ring, j + 1), at(ring + 1, j + 1)]);
                triangles.push([at(ring, j), at(ring + 1, j + 1), at(ring + 1, j)]);
            }
        }
        let tube = mesh(
            &[
                [1., 2., 0.],
                [-0.5, 2., 0.866],
                [-0.5, 2., -0.866],
                [1., 1., 0.],
                [-0.5, 1., 0.866],
                [-0.5, 1., -0.866],
                [1., 0., 0.],
                [-0.5, 0., 0.866],
                [-0.5, 0., -0.866],
            ],
            &triangles,
        )?;
        assert!(!tube.collapse_keeps_manifold(3, 4));
        Ok(())
    }

    #[test]
    fn orientation_check_rejects_moves_that_invert_a_face() -> Result<(), GeoError> {
        // A flat fan around vertex 4, in the XZ plane.
        let fan = mesh(
            &[
                [-1., 0., -1.],
                [1., 0., -1.],
                [1., 0., 1.],
                [-1., 0., 1.],
                [0., 0., 0.],
            ],
            &[[4, 1, 0], [4, 2, 1], [4, 3, 2], [4, 0, 3]],
        )?;
        // Merging the centre into a rim corner keeps the fan flat and consistently oriented.
        assert!(fan.collapse_keeps_orientation(4, 0, [-1.0, 0.0, -1.0]));
        // Placing the merged vertex past the far rim edges turns the surviving faces over.
        assert!(!fan.collapse_keeps_orientation(4, 0, [5.0, 0.0, 5.0]));
        Ok(())
    }

    #[test]
    fn local_operations_preserve_counts_and_orientation() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        // Two triangles forming a unit square, sharing diagonal (0, 2).
        let mut square = mesh(
            &[[0., 0., 0.], [0., 0., 1.], [1., 0., 1.], [1., 0., 0.]],
            &[[0, 1, 2], [0, 2, 3]],
        )?;
        assert!(square.flip_edge(0, 2));
        assert!(square.edge_triangles(0, 2).is_empty());
        assert_eq!(square.edge_triangles(1, 3).len(), 2);
        let flipped = square.to_geo(&ctx)?;
        assert!((f64::from(flipped.surface_area()) - 1.0).abs() < 1e-6);
        // Boundary edges cannot flip.
        assert!(!square.flip_edge(0, 1));

        let middle = square.split_edge(1, 3, [0.5, 0.0, 0.5]);
        assert_eq!(square.live_tris, 4);
        assert_eq!(square.neighbours(middle), vec![0, 1, 2, 3]);
        let split = square.to_geo(&ctx)?;
        assert!((f64::from(split.surface_area()) - 1.0).abs() < 1e-6);

        // Every face keeps pointing the same way after both operations.
        let normals: Vec<f64> = (0..square.tris.len())
            .filter(|&i| square.tri_alive[i])
            .map(|i| square.area_vector_with(square.tris[i], &[], [0.0; 3])[1])
            .collect();
        assert_eq!(normals.len(), 4);
        assert!(normals.iter().all(|&y| y * normals[0] > 0.0), "{normals:?}");
        Ok(())
    }
}
