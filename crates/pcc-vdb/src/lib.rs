//! Sparse $8^3$ Copy-on-Write brick signed distance fields (`VdbGrid`), generalized winding
//! number mesh voxelization (`VDBFromPolygons`), level-set CSG and morphological reshape
//! (`VDBReshape`, `VDBCombine`), and guaranteed-manifold Dual Surface Nets polygon extraction (`ConvertVDB`).

mod fast_winding;

use fast_winding::MeshField;
use pcc_geo::{
    AttrDomain, AttributeColumn, FiniteF32, GeoDetail, GeoError, PAGE_LEN, PrimKind, PrimRun,
    local_index, split_coord,
};
pub use pcc_geo::{BRICK_DIM, BRICK_LEN, Brick8x8x8, VdbGrid};
use pcc_math::Vec3;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VdbCombineOp {
    Union,
    Intersection,
    Difference,
    SmoothUnion { radius: f32 },
}

/// What can be done with a grid, beyond storing it.
pub trait VdbOps: Sized {
    /// Builds a narrow-band signed distance field (`VdbGrid`) from polygon primitives of `geo`
    /// using exact point-to-triangle distance and generalized solid-angle winding numbers.
    fn from_polygons_cancellable(
        geo: &GeoDetail,
        voxel_size: f32,
        half_band_voxels: u32,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError>;

    /// Applies a morphological level-set offset (`offset > 0` dilates the solid interior $\phi < 0$;
    /// `offset < 0` erodes/shrinks it) and optional 6-neighbor Laplacian smoothing iterations.
    fn reshape_cancellable(
        &self,
        offset: f32,
        smooth_iterations: u32,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError>;

    /// Combines two `VdbGrid` signed distance fields using level-set CSG (`Union`, `Intersection`, `Difference`, `SmoothUnion`).
    fn csg_combine_cancellable(
        &self,
        other: &Self,
        op: VdbCombineOp,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError>;

    /// Extracts the $\phi = 0$ isosurface of this `VdbGrid` as a guaranteed closed manifold
    /// all-quad `GeoDetail` (`is_closed_manifold() == true`) using Dual Surface Nets with
    /// checkerboard saddle regularization.
    fn to_polygons_cancellable(
        &self,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<GeoDetail, GeoError>;

    /// Serializes active narrow-band voxels of this `VdbGrid` into a `GeoDetail` point cloud
    /// with point attributes `f@phi`, `i@vx`, `i@vy`, `i@vz` and detail attributes `f@vdb_voxel_size`, `f@vdb_background`.
    fn to_voxel_geo_detail_cancellable(
        &self,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<GeoDetail, GeoError>;

    /// Reconstructs a `VdbGrid` from a voxel `GeoDetail` produced by `to_voxel_geo_detail_cancellable`.
    fn from_voxel_geo_detail_cancellable(
        geo: &GeoDetail,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError>;
}

impl VdbOps for VdbGrid {
    fn from_polygons_cancellable(
        geo: &GeoDetail,
        voxel_size: f32,
        half_band_voxels: u32,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        if !voxel_size.is_finite() || voxel_size <= 0.0 || half_band_voxels == 0 {
            return Err(GeoError::InvalidTopology);
        }
        let triangles = extract_triangles(geo)?;
        if triangles.is_empty() {
            return Err(GeoError::InvalidTopology);
        }
        let (bmin, bmax) = geo.bounds().ok_or(GeoError::InvalidTopology)?;
        let band = (half_band_voxels.max(2) as f32) * voxel_size;
        let bg = FiniteF32::checked(band)?;
        let vs = FiniteF32::checked(voxel_size)?;

        let min_vx = ((bmin.x - band) / voxel_size).floor() as i32;
        let min_vy = ((bmin.y - band) / voxel_size).floor() as i32;
        let min_vz = ((bmin.z - band) / voxel_size).floor() as i32;
        let max_vx = ((bmax.x + band) / voxel_size).ceil() as i32;
        let max_vy = ((bmax.y + band) / voxel_size).ceil() as i32;
        let max_vz = ((bmax.z + band) / voxel_size).ceil() as i32;

        let span_x =
            usize::try_from((max_vx - min_vx + 1).max(0)).map_err(|_| GeoError::LimitExceeded)?;
        let span_y =
            usize::try_from((max_vy - min_vy + 1).max(0)).map_err(|_| GeoError::LimitExceeded)?;
        let span_z =
            usize::try_from((max_vz - min_vz + 1).max(0)).map_err(|_| GeoError::LimitExceeded)?;
        let total_voxels = span_x
            .checked_mul(span_y)
            .and_then(|v| v.checked_mul(span_z))
            .ok_or(GeoError::LimitExceeded)?;
        let est_bytes = total_voxels
            .checked_mul(std::mem::size_of::<f32>())
            .ok_or(GeoError::LimitExceeded)?;
        if est_bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }

        let field = MeshField::build(triangles);
        let band_squared = band * band;
        let slices = evaluate_slices(min_vz..=max_vz, cancelled, |vz| {
            let mut slice = Vec::new();
            for vy in min_vy..=max_vy {
                if cancelled.load(Ordering::Acquire) {
                    return Err(GeoError::Cancelled);
                }
                for vx in min_vx..=max_vx {
                    let p = Vec3::checked(
                        (vx as f32 + 0.5) * voxel_size,
                        (vy as f32 + 0.5) * voxel_size,
                        (vz as f32 + 0.5) * voxel_size,
                    )?;
                    let dist = field.distance_squared_within(p, band_squared).sqrt();
                    // Winding magnitude, so that inside-out shells still read as solid.
                    let sign = if field.winding(p).abs() >= 0.5 {
                        -1.0
                    } else {
                        1.0
                    };
                    let phi = (sign * dist).clamp(-band, band);
                    if phi < band {
                        slice.push((vx, vy, FiniteF32::checked(phi)?));
                    }
                }
            }
            Ok(slice)
        })?;

        // Merging in slice order makes the result independent of thread scheduling.
        let mut brick_vecs: BTreeMap<(i32, i32, i32), Vec<FiniteF32>> = BTreeMap::new();
        for (vz, slice) in (min_vz..=max_vz).zip(slices) {
            for (vx, vy, phi) in slice {
                let (bx, lx) = split_coord(vx);
                let (by, ly) = split_coord(vy);
                let (bz, lz) = split_coord(vz);
                let entry = brick_vecs
                    .entry((bx, by, bz))
                    .or_insert_with(|| vec![bg; BRICK_LEN]);
                entry[local_index(lx, ly, lz)] = phi;
            }
        }

        let mut bricks = BTreeMap::new();
        for (coord, vals) in brick_vecs {
            bricks.insert(coord, Brick8x8x8::from_vec(vals)?);
        }
        Self::from_parts(vs, bg, bricks)
    }

    fn reshape_cancellable(
        &self,
        offset: f32,
        smooth_iterations: u32,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        if !offset.is_finite() {
            return Err(GeoError::NonFinite);
        }
        if smooth_iterations > 50 {
            return Err(GeoError::InvalidTopology);
        }
        let brick_bytes = self
            .bricks()
            .len()
            .checked_mul(BRICK_LEN * std::mem::size_of::<FiniteF32>())
            .ok_or(GeoError::LimitExceeded)?;
        if brick_bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }

        let bg = self.background();
        let mut current = self.clone();
        if offset != 0.0 {
            let mut next_bricks = BTreeMap::new();
            for (&coord, brick) in current.bricks() {
                if cancelled.load(Ordering::Acquire) {
                    return Err(GeoError::Cancelled);
                }
                let mut vals = Vec::with_capacity(BRICK_LEN);
                for &v in brick.values() {
                    let shifted = (v.get() - offset).clamp(-bg, bg);
                    vals.push(FiniteF32::checked(shifted)?);
                }
                next_bricks.insert(coord, Brick8x8x8::from_vec(vals)?);
            }
            current = current.with_bricks(next_bricks);
        }

        for _ in 0..smooth_iterations {
            if cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            let mut next_bricks = BTreeMap::new();
            for &(bx, by, bz) in current.bricks().keys() {
                let mut vals = vec![FiniteF32::checked(current.background())?; BRICK_LEN];
                for lz in 0..BRICK_DIM {
                    for ly in 0..BRICK_DIM {
                        for lx in 0..BRICK_DIM {
                            let vx = bx * (BRICK_DIM as i32) + (lx as i32);
                            let vy = by * (BRICK_DIM as i32) + (ly as i32);
                            let vz = bz * (BRICK_DIM as i32) + (lz as i32);
                            let c0 = current.get(vx, vy, vz);
                            let n_sum = current.get(vx - 1, vy, vz)
                                + current.get(vx + 1, vy, vz)
                                + current.get(vx, vy - 1, vz)
                                + current.get(vx, vy + 1, vz)
                                + current.get(vx, vy, vz - 1)
                                + current.get(vx, vy, vz + 1);
                            let smoothed = (0.5 * c0 + 0.5 * (n_sum / 6.0)).clamp(-bg, bg);
                            vals[local_index(lx, ly, lz)] = FiniteF32::checked(smoothed)?;
                        }
                    }
                }
                next_bricks.insert((bx, by, bz), Brick8x8x8::from_vec(vals)?);
            }
            current = current.with_bricks(next_bricks);
        }
        Ok(current)
    }

    fn csg_combine_cancellable(
        &self,
        other: &Self,
        op: VdbCombineOp,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        let mut all_bricks = BTreeSet::new();
        for &k in self.bricks().keys() {
            all_bricks.insert(k);
        }
        for &k in other.bricks().keys() {
            all_bricks.insert(k);
        }
        let bytes = all_bricks
            .len()
            .checked_mul(BRICK_LEN * std::mem::size_of::<FiniteF32>())
            .ok_or(GeoError::LimitExceeded)?;
        if bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }

        let bg = self.background().max(other.background());
        let bg_checked = FiniteF32::checked(bg)?;
        let mut out_bricks = BTreeMap::new();
        for (bx, by, bz) in all_bricks {
            if cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            let mut vals = vec![bg_checked; BRICK_LEN];
            let mut any_active = false;
            for lz in 0..BRICK_DIM {
                for ly in 0..BRICK_DIM {
                    for lx in 0..BRICK_DIM {
                        let vx = bx * (BRICK_DIM as i32) + (lx as i32);
                        let vy = by * (BRICK_DIM as i32) + (ly as i32);
                        let vz = bz * (BRICK_DIM as i32) + (lz as i32);
                        let a = self.get(vx, vy, vz);
                        let b = other.get(vx, vy, vz);
                        let combined = match op {
                            VdbCombineOp::Union => a.min(b),
                            VdbCombineOp::Intersection => a.max(b),
                            VdbCombineOp::Difference => a.max(-b),
                            VdbCombineOp::SmoothUnion { radius } => {
                                let k = radius.max(1e-4);
                                let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
                                b * (1.0 - h) + a * h - k * h * (1.0 - h)
                            }
                        }
                        .clamp(-bg, bg);
                        if combined < bg {
                            any_active = true;
                        }
                        vals[local_index(lx, ly, lz)] = FiniteF32::checked(combined)?;
                    }
                }
            }
            if any_active {
                out_bricks.insert((bx, by, bz), Brick8x8x8::from_vec(vals)?);
            }
        }
        Self::from_parts(
            FiniteF32::checked(self.voxel_size())?,
            bg_checked,
            out_bricks,
        )
    }

    fn to_polygons_cancellable(
        &self,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<GeoDetail, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        if self.bricks().is_empty() {
            return Ok(GeoDetail::points(Vec::new()));
        }

        // Collect all inside voxel coordinates (phi < 0.0)
        let mut inside: BTreeSet<(i32, i32, i32)> = BTreeSet::new();
        for (&(bx, by, bz), brick) in self.bricks() {
            for lz in 0..BRICK_DIM {
                for ly in 0..BRICK_DIM {
                    for lx in 0..BRICK_DIM {
                        if brick.values()[local_index(lx, ly, lz)].get() < 0.0 {
                            let vx = bx * (BRICK_DIM as i32) + (lx as i32);
                            let vy = by * (BRICK_DIM as i32) + (ly as i32);
                            let vz = bz * (BRICK_DIM as i32) + (lz as i32);
                            inside.insert((vx, vy, vz));
                        }
                    }
                }
            }
        }
        if inside.is_empty() {
            return Ok(GeoDetail::points(Vec::new()));
        }

        regularize_ambiguous_cells(&mut inside, cancelled)?;

        // Determine candidate dual cells: any cell (cx, cy, cz) whose 8 corners (cx+dx, cy+dy, cz+dz)
        // contain at least one inside and one outside voxel.
        let mut candidate_cells = BTreeSet::new();
        for &(vx, vy, vz) in &inside {
            for dz in -1..=0 {
                for dy in -1..=0 {
                    for dx in -1..=0 {
                        candidate_cells.insert((vx + dx, vy + dy, vz + dz));
                    }
                }
            }
        }

        let cell_edges: [([i32; 3], [i32; 3]); 12] = [
            ([0, 0, 0], [1, 0, 0]),
            ([0, 1, 0], [1, 1, 0]),
            ([0, 0, 1], [1, 0, 1]),
            ([0, 1, 1], [1, 1, 1]),
            ([0, 0, 0], [0, 1, 0]),
            ([1, 0, 0], [1, 1, 0]),
            ([0, 0, 1], [0, 1, 1]),
            ([1, 0, 1], [1, 1, 1]),
            ([0, 0, 0], [0, 0, 1]),
            ([1, 0, 0], [1, 0, 1]),
            ([0, 1, 0], [0, 1, 1]),
            ([1, 1, 0], [1, 1, 1]),
        ];

        let mut cell_to_pt: BTreeMap<(i32, i32, i32), u32> = BTreeMap::new();
        let mut points: Vec<Vec3> = Vec::new();

        for (cx, cy, cz) in candidate_cells {
            if points.len().is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            let mut sum = Vec3::ZERO;
            let mut count = 0u32;
            for (c0, c1) in cell_edges {
                let v0 = (cx + c0[0], cy + c0[1], cz + c0[2]);
                let v1 = (cx + c1[0], cy + c1[1], cz + c1[2]);
                let in0 = inside.contains(&v0);
                let in1 = inside.contains(&v1);
                if in0 != in1 {
                    let p0 = self.voxel_center(v0.0, v0.1, v0.2);
                    let p1 = self.voxel_center(v1.0, v1.1, v1.2);
                    let phi0 = self.get(v0.0, v0.1, v0.2);
                    let phi1 = self.get(v1.0, v1.1, v1.2);
                    let denom = phi0 - phi1;
                    let t = if denom.abs() > 1e-6 {
                        (phi0 / denom).clamp(0.1, 0.9)
                    } else {
                        0.5
                    };
                    let diff = p1.sub_checked(p0)?;
                    let crossing = p0.add_checked(diff.scale_checked(t)?)?;
                    sum = sum.add_checked(crossing)?;
                    count += 1;
                }
            }
            if count > 0 {
                let dual_pos = sum.scale_checked(1.0 / (count as f32))?;
                let pt_idx = u32::try_from(points.len()).map_err(|_| GeoError::LimitExceeded)?;
                points.push(dual_pos);
                cell_to_pt.insert((cx, cy, cz), pt_idx);
            }
        }

        // Emit 1 oriented quad per sign-changing grid edge
        let mut active_corners = BTreeSet::new();
        for &(vx, vy, vz) in &inside {
            active_corners.insert((vx, vy, vz));
            active_corners.insert((vx - 1, vy, vz));
            active_corners.insert((vx, vy - 1, vz));
            active_corners.insert((vx, vy, vz - 1));
        }

        let mut vertex_points: Vec<u32> = Vec::new();
        for (i, j, k) in active_corners {
            // +X edge: (i, j, k) -> (i + 1, j, k)
            let in0 = inside.contains(&(i, j, k));
            let in_x = inside.contains(&(i + 1, j, k));
            if in0 != in_x
                && let (Some(&c0), Some(&c1), Some(&c2), Some(&c3)) = (
                    cell_to_pt.get(&(i, j - 1, k - 1)),
                    cell_to_pt.get(&(i, j, k - 1)),
                    cell_to_pt.get(&(i, j, k)),
                    cell_to_pt.get(&(i, j - 1, k)),
                )
            {
                if in0 {
                    vertex_points.extend_from_slice(&[c0, c1, c2, c3]);
                } else {
                    vertex_points.extend_from_slice(&[c0, c3, c2, c1]);
                }
            }

            // +Y edge: (i, j, k) -> (i, j + 1, k)
            let in_y = inside.contains(&(i, j + 1, k));
            if in0 != in_y
                && let (Some(&c0), Some(&c1), Some(&c2), Some(&c3)) = (
                    cell_to_pt.get(&(i - 1, j, k - 1)),
                    cell_to_pt.get(&(i - 1, j, k)),
                    cell_to_pt.get(&(i, j, k)),
                    cell_to_pt.get(&(i, j, k - 1)),
                )
            {
                if in0 {
                    vertex_points.extend_from_slice(&[c0, c1, c2, c3]);
                } else {
                    vertex_points.extend_from_slice(&[c0, c3, c2, c1]);
                }
            }

            // +Z edge: (i, j, k) -> (i, j, k + 1)
            let in_z = inside.contains(&(i, j, k + 1));
            if in0 != in_z
                && let (Some(&c0), Some(&c1), Some(&c2), Some(&c3)) = (
                    cell_to_pt.get(&(i - 1, j - 1, k)),
                    cell_to_pt.get(&(i, j - 1, k)),
                    cell_to_pt.get(&(i, j, k)),
                    cell_to_pt.get(&(i - 1, j, k)),
                )
            {
                if in0 {
                    vertex_points.extend_from_slice(&[c0, c1, c2, c3]);
                } else {
                    vertex_points.extend_from_slice(&[c0, c3, c2, c1]);
                }
            }
        }

        let pt_bytes = points
            .len()
            .checked_mul(std::mem::size_of::<Vec3>())
            .ok_or(GeoError::LimitExceeded)?;
        let vtx_bytes = vertex_points
            .len()
            .checked_mul(std::mem::size_of::<u32>())
            .ok_or(GeoError::LimitExceeded)?;
        if pt_bytes.saturating_add(vtx_bytes) > memory_limit {
            return Err(GeoError::LimitExceeded);
        }

        let quad_count = vertex_points.len() / 4;
        let base = GeoDetail::points(points);
        if quad_count == 0 {
            Ok(base)
        } else {
            base.with_primitives(
                vec![PrimRun {
                    kind: PrimKind::Polygon,
                    arity: 4,
                    prim_count: quad_count,
                }],
                vertex_points,
            )
        }
    }

    fn to_voxel_geo_detail_cancellable(
        &self,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<GeoDetail, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        let bg = self.background();
        let mut points = Vec::new();
        let mut phi_vals = Vec::new();
        let mut vx_vals = Vec::new();
        let mut vy_vals = Vec::new();
        let mut vz_vals = Vec::new();

        for (&(bx, by, bz), brick) in self.bricks() {
            if cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            for lz in 0..BRICK_DIM {
                for ly in 0..BRICK_DIM {
                    for lx in 0..BRICK_DIM {
                        let phi = brick.values()[local_index(lx, ly, lz)].get();
                        if phi < bg {
                            let vx = bx * (BRICK_DIM as i32) + (lx as i32);
                            let vy = by * (BRICK_DIM as i32) + (ly as i32);
                            let vz = bz * (BRICK_DIM as i32) + (lz as i32);
                            points.push(self.voxel_center(vx, vy, vz));
                            phi_vals.push(phi);
                            vx_vals.push(vx);
                            vy_vals.push(vy);
                            vz_vals.push(vz);
                        }
                    }
                }
            }
        }
        let bytes = points
            .len()
            .checked_mul(std::mem::size_of::<Vec3>() + 4 * std::mem::size_of::<f32>())
            .ok_or(GeoError::LimitExceeded)?;
        if bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }

        GeoDetail::points(points)
            .with_attribute("phi", AttributeColumn::f32_checked(phi_vals)?)?
            .with_attribute("vx", AttributeColumn::i32_values(vx_vals))?
            .with_attribute("vy", AttributeColumn::i32_values(vy_vals))?
            .with_attribute("vz", AttributeColumn::i32_values(vz_vals))?
            .with_domain_attribute(
                AttrDomain::Detail,
                "vdb_voxel_size",
                AttributeColumn::f32_checked(vec![self.voxel_size()])?,
            )?
            .with_domain_attribute(
                AttrDomain::Detail,
                "vdb_background",
                AttributeColumn::f32_checked(vec![bg])?,
            )
    }

    fn from_voxel_geo_detail_cancellable(
        geo: &GeoDetail,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        let vs = geo
            .domain_attribute(AttrDomain::Detail, "vdb_voxel_size")
            .and_then(AttributeColumn::as_f32)
            .and_then(|c| c.get(0))
            .ok_or(GeoError::AttributeNotFound)?;
        let bg = geo
            .domain_attribute(AttrDomain::Detail, "vdb_background")
            .and_then(AttributeColumn::as_f32)
            .and_then(|c| c.get(0))
            .ok_or(GeoError::AttributeNotFound)?;
        let phi_col = geo
            .attribute("phi")
            .and_then(AttributeColumn::as_f32)
            .ok_or(GeoError::AttributeNotFound)?;
        let vx_col = geo
            .attribute("vx")
            .and_then(AttributeColumn::as_i32)
            .ok_or(GeoError::AttributeNotFound)?;
        let vy_col = geo
            .attribute("vy")
            .and_then(AttributeColumn::as_i32)
            .ok_or(GeoError::AttributeNotFound)?;
        let vz_col = geo
            .attribute("vz")
            .and_then(AttributeColumn::as_i32)
            .ok_or(GeoError::AttributeNotFound)?;

        let mut brick_vecs: BTreeMap<(i32, i32, i32), Vec<FiniteF32>> = BTreeMap::new();
        for i in 0..geo.len() {
            if i.is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            let phi = phi_col.get(i).ok_or(GeoError::IndexOutOfBounds)?;
            let vx = vx_col.get(i).ok_or(GeoError::IndexOutOfBounds)?;
            let vy = vy_col.get(i).ok_or(GeoError::IndexOutOfBounds)?;
            let vz = vz_col.get(i).ok_or(GeoError::IndexOutOfBounds)?;
            let (bx, lx) = split_coord(vx);
            let (by, ly) = split_coord(vy);
            let (bz, lz) = split_coord(vz);
            let entry = brick_vecs
                .entry((bx, by, bz))
                .or_insert_with(|| vec![bg; BRICK_LEN]);
            entry[local_index(lx, ly, lz)] = phi;
        }
        let bytes = brick_vecs
            .len()
            .checked_mul(BRICK_LEN * std::mem::size_of::<FiniteF32>())
            .ok_or(GeoError::LimitExceeded)?;
        if bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }
        let mut bricks = BTreeMap::new();
        for (coord, vals) in brick_vecs {
            bricks.insert(coord, Brick8x8x8::from_vec(vals)?);
        }
        Self::from_parts(vs, bg, bricks)
    }
}

/// Performs a guaranteed-manifold volumetric boolean (`Union`, `Intersection`, `Difference`)
/// on two polygon meshes `left` and `right` at `voxel_size`.
pub fn boolean_meshes_cancellable(
    left: &GeoDetail,
    right: &GeoDetail,
    op: VdbCombineOp,
    voxel_size: f32,
    memory_limit: usize,
    cancelled: &AtomicBool,
) -> Result<GeoDetail, GeoError> {
    let grid_a = VdbGrid::from_polygons_cancellable(left, voxel_size, 3, memory_limit, cancelled)?;
    let grid_b = VdbGrid::from_polygons_cancellable(right, voxel_size, 3, memory_limit, cancelled)?;
    let combined = grid_a.csg_combine_cancellable(&grid_b, op, memory_limit, cancelled)?;
    combined.to_polygons_cancellable(memory_limit, cancelled)
}

/// Runs `evaluate` for every slice on all available cores, returning results in slice order.
fn evaluate_slices<T: Send>(
    slices: std::ops::RangeInclusive<i32>,
    cancelled: &AtomicBool,
    evaluate: impl Fn(i32) -> Result<T, GeoError> + Sync,
) -> Result<Vec<T>, GeoError> {
    let all: Vec<i32> = slices.collect();
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(all.len().max(1));
    let run_worker = || -> Result<Vec<(usize, T)>, GeoError> {
        let mut done = Vec::new();
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(&slice) = all.get(index) else {
                return Ok(done);
            };
            if cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            done.push((index, evaluate(slice)?));
        }
    };
    let batches: Vec<Result<Vec<(usize, T)>, GeoError>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers).map(|_| scope.spawn(run_worker)).collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or(Err(GeoError::LimitExceeded)))
            .collect()
    });
    let mut indexed = Vec::with_capacity(all.len());
    for batch in batches {
        indexed.extend(batch?);
    }
    indexed.sort_by_key(|&(index, _)| index);
    Ok(indexed.into_iter().map(|(_, value)| value).collect())
}

/// Groups the set bits of a cube-corner mask into components connected along cube edges.
fn corner_components(mask: u8) -> Vec<u8> {
    let mut remaining = mask;
    let mut components = Vec::new();
    while remaining != 0 {
        let mut component = remaining & remaining.wrapping_neg();
        loop {
            let mut grown = component;
            for corner in 0..8u8 {
                if component & (1 << corner) != 0 {
                    for axis in [1u8, 2, 4] {
                        grown |= (1 << (corner ^ axis)) & mask;
                    }
                }
            }
            if grown == component {
                break;
            }
            component = grown;
        }
        components.push(component);
        remaining &= !component;
    }
    components
}

/// Corners to mark inside so that this cell holds exactly one surface sheet, or 0 if it already
/// does. Inside corners that are split fill their bounding sub-box; split outside corners fill all
/// but the largest group. Neither reaches past the bounds of the inside corners already present
/// in a way that can grow without limit, because a full cell has no surface left to pinch.
fn cell_repair(mask: u8) -> u8 {
    let inside = corner_components(mask);
    if inside.len() > 1 {
        let (mut low, mut high) = (7u8, 0u8);
        for corner in 0..8u8 {
            if mask & (1 << corner) != 0 {
                low &= corner;
                high |= corner;
            }
        }
        let mut fill = 0u8;
        for corner in 0..8u8 {
            if corner & !high == 0 && low & !corner == 0 {
                fill |= 1 << corner;
            }
        }
        return fill & !mask;
    }
    let mut outside = corner_components(!mask);
    if outside.len() > 1 {
        outside.sort_by_key(|group| (std::cmp::Reverse(group.count_ones()), *group));
        return outside[1..].iter().fold(0, |fill, group| fill | group);
    }
    0
}

/// Fills voxels until every grid cell holds at most one surface sheet: its inside corners are one
/// edge-connected group, and so are its outside corners.
///
/// This is exactly the condition under which Surface Nets is manifold. Each output vertex then has
/// a single closed fan of quads, because the boundary between the two groups is one loop on the
/// cell. Each output edge has exactly two quads, because a checkerboard grid face, the only face
/// with four crossings, has split corners and cannot survive. Voxels are only ever added and only
/// within finitely many cells, so the loop terminates.
fn regularize_ambiguous_cells(
    inside: &mut BTreeSet<(i32, i32, i32)>,
    cancelled: &AtomicBool,
) -> Result<(), GeoError> {
    let repairs: Vec<u8> = (0..=255u8).map(cell_repair).collect();
    let cells_touching = |voxel: (i32, i32, i32), cells: &mut BTreeSet<(i32, i32, i32)>| {
        for dz in -1..=0 {
            for dy in -1..=0 {
                for dx in -1..=0 {
                    cells.insert((voxel.0 + dx, voxel.1 + dy, voxel.2 + dz));
                }
            }
        }
    };
    let mut pending = BTreeSet::new();
    for &voxel in inside.iter() {
        cells_touching(voxel, &mut pending);
    }
    while !pending.is_empty() {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        let mut added = Vec::new();
        for &(cx, cy, cz) in &pending {
            let corner_at = |corner: u8| {
                (
                    cx + i32::from(corner & 1),
                    cy + i32::from((corner >> 1) & 1),
                    cz + i32::from((corner >> 2) & 1),
                )
            };
            let mut mask = 0u8;
            for corner in 0..8u8 {
                if inside.contains(&corner_at(corner)) {
                    mask |= 1 << corner;
                }
            }
            let fill = repairs[mask as usize];
            for corner in 0..8u8 {
                if fill & (1 << corner) != 0 {
                    added.push(corner_at(corner));
                }
            }
        }
        // Applying a whole pass at once keeps the result independent of visit order.
        pending.clear();
        for voxel in added {
            if inside.insert(voxel) {
                cells_touching(voxel, &mut pending);
            }
        }
    }
    Ok(())
}

fn extract_triangles(geo: &GeoDetail) -> Result<Vec<(Vec3, Vec3, Vec3)>, GeoError> {
    let mut tris = Vec::new();
    let mut vtx_cursor = 0usize;
    for run in geo.prim_runs() {
        let arity = run.arity as usize;
        if run.kind == PrimKind::Polygon && arity >= 3 {
            for _ in 0..run.prim_count {
                let p0_idx = geo
                    .vertex_point(vtx_cursor)
                    .ok_or(GeoError::InvalidTopology)? as usize;
                let p0 = geo.point(p0_idx).ok_or(GeoError::IndexOutOfBounds)?;
                for k in 1..(arity - 1) {
                    let p1_idx =
                        geo.vertex_point(vtx_cursor + k)
                            .ok_or(GeoError::InvalidTopology)? as usize;
                    let p2_idx =
                        geo.vertex_point(vtx_cursor + k + 1)
                            .ok_or(GeoError::InvalidTopology)? as usize;
                    let p1 = geo.point(p1_idx).ok_or(GeoError::IndexOutOfBounds)?;
                    let p2 = geo.point(p2_idx).ok_or(GeoError::IndexOutOfBounds)?;
                    tris.push((p0, p1, p2));
                }
                vtx_cursor += arity;
            }
        } else {
            vtx_cursor += arity * run.prim_count;
        }
    }
    Ok(tris)
}

fn point_triangle_dist_sq(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let ab = b.sub_checked(a).unwrap_or(Vec3::ZERO);
    let ac = c.sub_checked(a).unwrap_or(Vec3::ZERO);
    let ap = p.sub_checked(a).unwrap_or(Vec3::ZERO);

    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.dot(ap);
    }

    let bp = p.sub_checked(b).unwrap_or(Vec3::ZERO);
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return bp.dot(bp);
    }

    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        let proj = a
            .add_checked(ab.scale_checked(v).unwrap_or(Vec3::ZERO))
            .unwrap_or(a);
        let diff = p.sub_checked(proj).unwrap_or(Vec3::ZERO);
        return diff.dot(diff);
    }

    let cp = p.sub_checked(c).unwrap_or(Vec3::ZERO);
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return cp.dot(cp);
    }

    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        let proj = a
            .add_checked(ac.scale_checked(w).unwrap_or(Vec3::ZERO))
            .unwrap_or(a);
        let diff = p.sub_checked(proj).unwrap_or(Vec3::ZERO);
        return diff.dot(diff);
    }

    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        let bc = c.sub_checked(b).unwrap_or(Vec3::ZERO);
        let proj = b
            .add_checked(bc.scale_checked(w).unwrap_or(Vec3::ZERO))
            .unwrap_or(b);
        let diff = p.sub_checked(proj).unwrap_or(Vec3::ZERO);
        return diff.dot(diff);
    }

    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    let proj = a
        .add_checked(ab.scale_checked(v).unwrap_or(Vec3::ZERO))
        .and_then(|q| q.add_checked(ac.scale_checked(w).unwrap_or(Vec3::ZERO)))
        .unwrap_or(a);
    let diff = p.sub_checked(proj).unwrap_or(Vec3::ZERO);
    diff.dot(diff)
}

/// Evaluates the exact generalized winding number $w(\mathbf{p}) = \frac{1}{4\pi}\sum \Omega_i(\mathbf{p})$
/// using the Van Oosterom & Strackee (1983) / Barill et al. (SIGGRAPH 2018) solid angle formula.
fn generalized_winding_number(p: Vec3, triangles: &[(Vec3, Vec3, Vec3)]) -> f32 {
    let mut sum_omega = 0.0f32;
    for &(v0, v1, v2) in triangles {
        let a = v0.sub_checked(p).unwrap_or(Vec3::ZERO);
        let b = v1.sub_checked(p).unwrap_or(Vec3::ZERO);
        let c = v2.sub_checked(p).unwrap_or(Vec3::ZERO);
        let la = a.length();
        let lb = b.length();
        let lc = c.length();
        if la < 1e-7 || lb < 1e-7 || lc < 1e-7 {
            return 1.0;
        }
        let cross_bc = b.cross_checked(c).unwrap_or(Vec3::ZERO);
        let numer = a.dot(cross_bc);
        let denom = la * lb * lc + a.dot(b) * lc + b.dot(c) * la + c.dot(a) * lb;
        sum_omega += 2.0 * numer.atan2(denom);
    }
    sum_omega / (4.0 * std::f32::consts::PI)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_box(min: Vec3, max: Vec3) -> Result<GeoDetail, GeoError> {
        let pts = vec![
            Vec3::checked(min.x, min.y, min.z)?,
            Vec3::checked(max.x, min.y, min.z)?,
            Vec3::checked(max.x, max.y, min.z)?,
            Vec3::checked(min.x, max.y, min.z)?,
            Vec3::checked(min.x, min.y, max.z)?,
            Vec3::checked(max.x, min.y, max.z)?,
            Vec3::checked(max.x, max.y, max.z)?,
            Vec3::checked(min.x, max.y, max.z)?,
        ];
        let vtx = vec![
            0, 3, 2, 1, // -Z
            4, 5, 6, 7, // +Z
            0, 1, 5, 4, // -Y
            3, 7, 6, 2, // +Y
            0, 4, 7, 3, // -X
            1, 2, 6, 5, // +X
        ];
        GeoDetail::points(pts).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 4,
                prim_count: 6,
            }],
            vtx,
        )
    }

    #[test]
    fn sparse_8x8x8_bricks_preserve_cow_sharing_and_hash() {
        let g0_res = VdbGrid::new(0.5, 2.0)
            .and_then(|g| g.with_voxel(1, 1, 1, -0.5))
            .and_then(|g| g.with_voxel(10, 1, 1, -0.25));
        assert!(g0_res.is_ok());
        let Ok(g0) = g0_res else {
            return;
        };
        assert_eq!(g0.active_brick_count(), 2);

        // Mutating voxel (2, 2, 2) in brick (0, 0, 0) copies only brick (0, 0, 0)
        // and preserves Arc pointer sharing on brick (1, 0, 0).
        let g1_res = g0.with_voxel(2, 2, 2, -0.75);
        assert!(g1_res.is_ok());
        if let Ok(g1) = g1_res {
            assert!(!g1.brick_shared_with(&g0, (0, 0, 0)));
            assert!(g1.brick_shared_with(&g0, (1, 0, 0)));
            assert_ne!(g1.hash(), g0.hash());
        }
    }

    #[test]
    fn vdb_from_polygons_reshape_convert_and_csg_booleans_produce_closed_manifolds() {
        let paused = AtomicBool::new(false);
        let Ok(box_a) = make_box(
            Vec3::ZERO,
            Vec3::checked(2.0, 2.0, 2.0).unwrap_or(Vec3::ZERO),
        ) else {
            return;
        };
        let Ok(box_b) = make_box(
            Vec3::checked(1.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(3.0, 2.0, 2.0).unwrap_or(Vec3::ZERO),
        ) else {
            return;
        };

        let vdb_res = VdbGrid::from_polygons_cancellable(&box_a, 0.5, 2, 4 << 20, &paused);
        assert!(vdb_res.is_ok());
        let Ok(vdb_a) = vdb_res else {
            return;
        };

        let surf_a = vdb_a.to_polygons_cancellable(4 << 20, &paused);
        assert!(surf_a.is_ok());
        let Ok(mesh_a) = surf_a else {
            return;
        };
        assert!(mesh_a.topology_index().is_closed_manifold());
        let vol_a = mesh_a.enclosed_volume();
        assert!(vol_a > 4.0 && vol_a < 10.0);

        // Dilate by +0.25 increases enclosed volume
        let dilated_res = vdb_a
            .reshape_cancellable(0.25, 1, 4 << 20, &paused)
            .and_then(|g| g.to_polygons_cancellable(4 << 20, &paused));
        assert!(dilated_res.is_ok());
        if let Ok(dilated) = dilated_res {
            assert!(dilated.topology_index().is_closed_manifold());
            assert!(dilated.enclosed_volume() > vol_a);
        }

        // Volumetric CSG Union, Difference, and Intersection on overlapping boxes
        let union_res =
            boolean_meshes_cancellable(&box_a, &box_b, VdbCombineOp::Union, 0.5, 4 << 20, &paused);
        let diff_res = boolean_meshes_cancellable(
            &box_a,
            &box_b,
            VdbCombineOp::Difference,
            0.5,
            4 << 20,
            &paused,
        );
        let inter_res = boolean_meshes_cancellable(
            &box_a,
            &box_b,
            VdbCombineOp::Intersection,
            0.5,
            4 << 20,
            &paused,
        );
        assert!(union_res.is_ok());
        assert!(diff_res.is_ok());
        assert!(inter_res.is_ok());
        if let (Ok(u_mesh), Ok(d_mesh), Ok(i_mesh)) = (union_res, diff_res, inter_res) {
            assert!(u_mesh.topology_index().is_closed_manifold());
            assert!(d_mesh.topology_index().is_closed_manifold());
            assert!(i_mesh.topology_index().is_closed_manifold());
            let vol_u = u_mesh.enclosed_volume();
            let vol_d = d_mesh.enclosed_volume();
            let vol_i = i_mesh.enclosed_volume();
            assert!(vol_u > vol_a);
            assert!(vol_d > 0.5 && vol_d < vol_a);
            assert!(vol_i > 0.5 && vol_i < vol_a);
        }
    }

    fn assert_surface_is_manifold(grid: &VdbGrid, label: &str) -> Result<GeoDetail, GeoError> {
        let paused = AtomicBool::new(false);
        let mesh = grid.to_polygons_cancellable(1 << 28, &paused)?;
        let topology = mesh.topology_index();
        assert!(
            topology.is_closed_manifold(),
            "{label}: {} boundary, {} non-manifold edges",
            topology.boundary_edge_count(),
            topology.non_manifold_edge_count()
        );
        assert_eq!(topology.non_manifold_vertex_count(&mesh), 0, "{label}");
        assert!(pcc_sop::signed_volume(&mesh) > 0.0, "{label}: inside out");
        Ok(mesh)
    }

    #[test]
    fn surface_nets_is_manifold_on_every_ambiguous_cell_configuration() -> Result<(), GeoError> {
        // All 254 non-trivial corner assignments of one cell, as the worst possible neighbourhood.
        for mask in 1..=254u8 {
            let mut grid = VdbGrid::new(1.0, 3.0)?;
            for corner in 0..8i32 {
                if mask & (1 << corner) != 0 {
                    grid =
                        grid.with_voxel(corner & 1, (corner >> 1) & 1, (corner >> 2) & 1, -1.0)?;
                }
            }
            assert_surface_is_manifold(&grid, &format!("mask {mask:#010b}"))?;
        }
        Ok(())
    }

    #[test]
    fn surface_nets_is_manifold_on_random_voxel_noise() -> Result<(), GeoError> {
        let rng = pcc_math::Philox4x32::new(0xD00D);
        for (round, density) in [0.05f32, 0.2, 0.35, 0.5, 0.65, 0.8, 0.95]
            .into_iter()
            .cycle()
            .take(70)
            .enumerate()
        {
            let mut grid = VdbGrid::new(1.0, 3.0)?;
            for x in 0..10i32 {
                for y in 0..10i32 {
                    for z in 0..10i32 {
                        let u = rng.generate_unit_f32([x as u32, y as u32, z as u32, round as u32]);
                        if u[0] < density {
                            grid = grid.with_voxel(x, y, z, -1.0)?;
                        }
                    }
                }
            }
            assert_surface_is_manifold(&grid, &format!("round {round} density {density}"))?;
        }
        Ok(())
    }

    #[test]
    fn cell_repair_leaves_single_sheet_cells_alone_and_splits_nothing_new() {
        for mask in 0..=255u8 {
            let single = corner_components(mask).len() <= 1 && corner_components(!mask).len() <= 1;
            let repair = cell_repair(mask);
            assert_eq!(repair == 0, single, "mask {mask:#010b}");
            assert_eq!(
                repair & mask,
                0,
                "mask {mask:#010b} refilled an inside corner"
            );
        }
        // Opposite body corners fill the whole cell; a face diagonal fills only its face.
        assert_eq!(cell_repair(0b1000_0001), 0b0111_1110);
        assert_eq!(cell_repair(0b0000_1001), 0b0000_0110);
        // Split outside corners fill the smaller group, keeping the output closest to the input.
        assert_eq!(cell_repair(!0b1000_0011), 0b1000_0000);
        // Equal groups break the tie deterministically: the lowest group stays outside.
        assert_eq!(cell_repair(!0b1000_0001), 0b1000_0000);
    }

    #[test]
    fn voxelization_is_independent_of_scheduling_and_inside_out_shells_are_solid()
    -> Result<(), GeoError> {
        let paused = AtomicBool::new(false);
        let ctx = pcc_sop::SopContext::new(1 << 28, &paused);
        let sphere = pcc_sop::create_sphere(1.0, 16, 24, &ctx)?;
        let first = VdbGrid::from_polygons_cancellable(&sphere, 0.2, 3, 1 << 28, &paused)?;
        for _ in 0..4 {
            let again = VdbGrid::from_polygons_cancellable(&sphere, 0.2, 3, 1 << 28, &paused)?;
            assert_eq!(first.hash(), again.hash());
        }
        let inverted = sphere.with_reversed_winding()?;
        let flipped = VdbGrid::from_polygons_cancellable(&inverted, 0.2, 3, 1 << 28, &paused)?;
        let mesh = assert_surface_is_manifold(&flipped, "inverted sphere")?;
        let exact = 4.0 / 3.0 * std::f64::consts::PI;
        assert!((pcc_sop::signed_volume(&mesh) - exact).abs() < 0.1 * exact);
        Ok(())
    }
    #[test]
    fn voxels_land_at_the_correct_height_on_an_asymmetric_solid() -> Result<(), GeoError> {
        let paused = AtomicBool::new(false);
        // A tetrahedron with its broad base at z = 0 and apex at z = 4. Mirroring the slices about
        // the grid's middle would keep a symmetric shape intact, but not this one.
        let v = |x, y, z| Vec3::checked(x, y, z);
        let wedge = GeoDetail::points(vec![
            v(-3.0, -3.0, 0.0)?,
            v(3.0, -3.0, 0.0)?,
            v(0.0, 3.0, 0.0)?,
            v(0.0, -1.0, 4.0)?,
        ])
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 4,
            }],
            vec![0, 2, 1, 0, 1, 3, 1, 2, 3, 2, 0, 3],
        )?;
        let grid = VdbGrid::from_polygons_cancellable(&wedge, 0.25, 3, 1 << 28, &paused)?;
        let phi_at = |x: f32, y: f32, z: f32| {
            let voxel = |c: f32| (c / 0.25).floor() as i32;
            grid.get(voxel(x), voxel(y), voxel(z))
        };
        assert!(
            phi_at(0.0, -1.0, 0.5) < 0.0,
            "just above the base is inside"
        );
        assert!(phi_at(0.0, -1.0, 3.0) < 0.0, "under the apex is inside");
        assert!(
            phi_at(2.0, 1.5, 3.0) > 0.0,
            "the same height off-axis is outside"
        );
        assert!(phi_at(0.0, -1.0, -1.0) > 0.0, "below the base is outside");
        Ok(())
    }
}
