//! Storage for sparse $8^3$ Copy-on-Write brick volumes, and the volume primitive that holds one.
//! It lives here so that geometry can hold a grid; `pcc-vdb` holds the algorithms that work on one.
//!
//! A volume primitive has one vertex, like a packed instance, and its grid sits in a side table in
//! primitive order. Its point only anchors the primitive: the grid's own coordinates say where its
//! voxels are, so moving the point would not move them, and operators that move points refuse it.

use crate::{FiniteF32, GeoDetail, GeoError, PrimKind, PrimRun};
use pcc_math::Vec3;
use std::{collections::BTreeMap, sync::Arc};

/// Logical brick edge length ($8 \times 8 \times 8 = 512$ voxels per sparse leaf brick).
pub const BRICK_DIM: usize = 8;
pub const BRICK_LEN: usize = BRICK_DIM * BRICK_DIM * BRICK_DIM;

#[derive(Clone, Debug)]
pub struct Brick8x8x8 {
    values: Arc<[FiniteF32]>,
    hash: blake3::Hash,
}

impl Brick8x8x8 {
    pub fn from_vec(values: Vec<FiniteF32>) -> Result<Self, GeoError> {
        if values.len() != BRICK_LEN {
            return Err(GeoError::AttributeLengthMismatch {
                expected: BRICK_LEN,
                actual: values.len(),
            });
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pcc-vdb-brick-v1");
        for v in &values {
            hasher.update(&v.get().to_bits().to_le_bytes());
        }
        Ok(Self {
            values: Arc::from(values),
            hash: hasher.finalize(),
        })
    }

    #[must_use]
    pub fn filled(value: FiniteF32) -> Self {
        let values = vec![value; BRICK_LEN];
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pcc-vdb-brick-v1");
        for v in &values {
            hasher.update(&v.get().to_bits().to_le_bytes());
        }
        Self {
            values: Arc::from(values),
            hash: hasher.finalize(),
        }
    }

    #[must_use]
    pub const fn hash(&self) -> blake3::Hash {
        self.hash
    }

    /// The voxel values, indexed by [`local_index`].
    #[must_use]
    pub fn values(&self) -> &[FiniteF32] {
        &self.values
    }
}

/// Sparse $8^3$ Copy-on-Write brick volumetric grid.
#[derive(Clone, Debug)]
pub struct VdbGrid {
    voxel_size: FiniteF32,
    background: FiniteF32,
    bricks: Arc<BTreeMap<(i32, i32, i32), Brick8x8x8>>,
}

pub fn split_coord(v: i32) -> (i32, usize) {
    let bx = v.div_euclid(BRICK_DIM as i32);
    let lx = v.rem_euclid(BRICK_DIM as i32) as usize;
    (bx, lx)
}

pub fn local_index(lx: usize, ly: usize, lz: usize) -> usize {
    (lz * BRICK_DIM + ly) * BRICK_DIM + lx
}

impl VdbGrid {
    pub fn new(voxel_size: f32, background: f32) -> Result<Self, GeoError> {
        let vs = FiniteF32::checked(voxel_size)?;
        let bg = FiniteF32::checked(background)?;
        if vs.get() <= 0.0 || bg.get() <= 0.0 {
            return Err(GeoError::InvalidTopology);
        }
        Ok(Self {
            voxel_size: vs,
            background: bg,
            bricks: Arc::new(BTreeMap::new()),
        })
    }

    #[must_use]
    pub const fn voxel_size(&self) -> f32 {
        self.voxel_size.get()
    }

    #[must_use]
    pub const fn background(&self) -> f32 {
        self.background.get()
    }

    #[must_use]
    pub fn active_brick_count(&self) -> usize {
        self.bricks.len()
    }

    #[must_use]
    pub fn get(&self, vx: i32, vy: i32, vz: i32) -> f32 {
        let (bx, lx) = split_coord(vx);
        let (by, ly) = split_coord(vy);
        let (bz, lz) = split_coord(vz);
        self.bricks
            .get(&(bx, by, bz))
            .and_then(|b| b.values.get(local_index(lx, ly, lz)))
            .map_or_else(|| self.background.get(), |v| v.get())
    }

    /// Returns a new `VdbGrid` with voxel `(vx, vy, vz)` set to `value`, copying only the
    /// affected $8^3$ brick and sharing all other bricks via `Arc`.
    pub fn with_voxel(&self, vx: i32, vy: i32, vz: i32, value: f32) -> Result<Self, GeoError> {
        let checked = FiniteF32::checked(value)?;
        let (bx, lx) = split_coord(vx);
        let (by, ly) = split_coord(vy);
        let (bz, lz) = split_coord(vz);
        let key = (bx, by, bz);
        let idx = local_index(lx, ly, lz);

        let mut next_bricks = (*self.bricks).clone();
        let mut vals = next_bricks
            .get(&key)
            .map_or_else(|| vec![self.background; BRICK_LEN], |b| b.values.to_vec());
        if let Some(slot) = vals.get_mut(idx) {
            *slot = checked;
        } else {
            return Err(GeoError::IndexOutOfBounds);
        }
        next_bricks.insert(key, Brick8x8x8::from_vec(vals)?);
        Ok(Self {
            voxel_size: self.voxel_size,
            background: self.background,
            bricks: Arc::new(next_bricks),
        })
    }

    #[must_use]
    pub fn brick_shared_with(&self, other: &Self, brick_coord: (i32, i32, i32)) -> bool {
        self.bricks.get(&brick_coord).is_some_and(|a| {
            other
                .bricks
                .get(&brick_coord)
                .is_some_and(|b| Arc::ptr_eq(&a.values, &b.values))
        })
    }

    #[must_use]
    pub fn hash(&self) -> blake3::Hash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pcc-vdb-grid-v1");
        hasher.update(&self.voxel_size.get().to_bits().to_le_bytes());
        hasher.update(&self.background.get().to_bits().to_le_bytes());
        hasher.update(&(self.bricks.len() as u64).to_le_bytes());
        for (&(bx, by, bz), brick) in &*self.bricks {
            hasher.update(&bx.to_le_bytes());
            hasher.update(&by.to_le_bytes());
            hasher.update(&bz.to_le_bytes());
            hasher.update(brick.hash.as_bytes());
        }
        hasher.finalize()
    }

    #[must_use]
    pub fn voxel_center(&self, vx: i32, vy: i32, vz: i32) -> Vec3 {
        let vs = self.voxel_size.get();
        Vec3::checked(
            (vx as f32 + 0.5) * vs,
            (vy as f32 + 0.5) * vs,
            (vz as f32 + 0.5) * vs,
        )
        .unwrap_or(Vec3::ZERO)
    }

    /// Assembles a grid from existing bricks, under the same rules as [`Self::new`].
    pub fn from_parts(
        voxel_size: FiniteF32,
        background: FiniteF32,
        bricks: BTreeMap<(i32, i32, i32), Brick8x8x8>,
    ) -> Result<Self, GeoError> {
        if voxel_size.get() <= 0.0 || background.get() <= 0.0 {
            return Err(GeoError::InvalidTopology);
        }
        Ok(Self {
            voxel_size,
            background,
            bricks: Arc::new(bricks),
        })
    }

    /// The same grid settings holding different bricks.
    #[must_use]
    pub fn with_bricks(&self, bricks: BTreeMap<(i32, i32, i32), Brick8x8x8>) -> Self {
        Self {
            voxel_size: self.voxel_size,
            background: self.background,
            bricks: Arc::new(bricks),
        }
    }

    /// How many voxels differ from the background.
    #[must_use]
    pub fn active_voxel_count(&self) -> usize {
        let background = self.background.get();
        self.bricks
            .values()
            .map(|brick| brick.values.iter().filter(|v| v.get() < background).count())
            .sum()
    }

    /// World-space bounds of the voxels that differ from the background, or `None` if there are no
    /// such voxels. Fails if a voxel lies beyond what a finite position can express.
    pub fn active_bounds(&self) -> Result<Option<(Vec3, Vec3)>, GeoError> {
        let background = self.background.get();
        let mut range: Option<([i64; 3], [i64; 3])> = None;
        for (&(bx, by, bz), brick) in &*self.bricks {
            for (index, value) in brick.values.iter().enumerate() {
                if value.get() >= background {
                    continue;
                }
                let dim = BRICK_DIM as i64;
                let local = index as i64;
                let voxel = [
                    i64::from(bx) * dim + local % dim,
                    i64::from(by) * dim + (local / dim) % dim,
                    i64::from(bz) * dim + local / (dim * dim),
                ];
                let (low, high) = range.get_or_insert((voxel, voxel));
                for axis in 0..3 {
                    low[axis] = low[axis].min(voxel[axis]);
                    high[axis] = high[axis].max(voxel[axis]);
                }
            }
        }
        let Some((low, high)) = range else {
            return Ok(None);
        };
        // A voxel fills the cell from its index to the next one.
        let size = f64::from(self.voxel_size.get());
        let at = |[x, y, z]: [i64; 3], offset: i64| {
            let world = |v: i64| ((v + offset) as f64 * size) as f32;
            Vec3::checked(world(x), world(y), world(z))
        };
        Ok(Some((at(low, 0)?, at(high, 1)?)))
    }

    /// Every allocated brick, keyed by brick coordinate.
    #[must_use]
    pub fn bricks(&self) -> &BTreeMap<(i32, i32, i32), Brick8x8x8> {
        &self.bricks
    }
}

/// A grid held by a volume primitive, with facts about it that would take a full scan to find.
#[derive(Clone, Debug)]
pub(crate) struct VolumeEntry {
    pub(crate) grid: Arc<VdbGrid>,
    pub(crate) bounds: Option<(Vec3, Vec3)>,
}

impl VolumeEntry {
    fn new(grid: Arc<VdbGrid>) -> Result<Self, GeoError> {
        Ok(Self {
            bounds: grid.active_bounds()?,
            grid,
        })
    }
}

impl GeoDetail {
    /// One volume primitive per grid, each anchored at the centre of its active voxels.
    pub fn from_volumes(grids: Vec<Arc<VdbGrid>>) -> Result<Self, GeoError> {
        let entries = grids
            .into_iter()
            .map(VolumeEntry::new)
            .collect::<Result<Vec<_>, _>>()?;
        let anchors = entries
            .iter()
            .map(|entry| {
                entry.bounds.map_or(Ok(Vec3::ZERO), |(low, high)| {
                    let middle = |a: f32, b: f32| (f64::from(a) + f64::from(b)) / 2.0;
                    Vec3::checked(
                        middle(low.x, high.x) as f32,
                        middle(low.y, high.y) as f32,
                        middle(low.z, high.z) as f32,
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let count = entries.len();
        let runs = if count == 0 {
            Vec::new()
        } else {
            vec![PrimRun {
                kind: PrimKind::Volume,
                arity: 1,
                prim_count: count,
            }]
        };
        let vertices = (0..count)
            .map(|at| u32::try_from(at).map_err(|_| GeoError::LimitExceeded))
            .collect::<Result<Vec<_>, _>>()?;
        let mut geo = Self::points(anchors).with_runs(runs, vertices)?;
        geo.volumes = (count > 0).then(|| Arc::from(entries));
        Ok(geo)
    }

    /// How many volume primitives this geometry holds.
    #[must_use]
    pub fn volume_count(&self) -> usize {
        self.volumes.as_ref().map_or(0, |entries| entries.len())
    }

    /// One grid, by its ordinal among the volume primitives.
    #[must_use]
    pub fn volume(&self, index: usize) -> Option<&Arc<VdbGrid>> {
        self.volumes.as_ref()?.get(index).map(|entry| &entry.grid)
    }

    /// Replaces every grid, keeping points and attributes. Also how a file's grids are attached,
    /// so there must be exactly one for each volume primitive.
    pub fn with_volume_grids(&self, grids: Vec<Arc<VdbGrid>>) -> Result<Self, GeoError> {
        let expected: usize = self
            .prim_runs
            .iter()
            .filter(|run| run.kind == PrimKind::Volume)
            .map(|run| run.prim_count)
            .sum();
        if grids.len() != expected {
            return Err(GeoError::InvalidTopology);
        }
        let entries = grids
            .into_iter()
            .map(VolumeEntry::new)
            .collect::<Result<Vec<_>, _>>()?;
        let mut next = self.clone();
        next.volumes = (expected > 0).then(|| Arc::from(entries));
        Ok(next)
    }

    /// Bounds that contain every volume's active voxels, without scanning any grid.
    pub(crate) fn volume_bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut bounds: Option<(Vec3, Vec3)> = None;
        for entry in self.volumes.as_deref().unwrap_or(&[]) {
            let Some((low, high)) = entry.bounds else {
                continue;
            };
            bounds = Some(bounds.map_or((low, high), |(lo, hi)| {
                (
                    Vec3 {
                        x: lo.x.min(low.x),
                        y: lo.y.min(low.y),
                        z: lo.z.min(low.z),
                    },
                    Vec3 {
                        x: hi.x.max(high.x),
                        y: hi.y.max(high.y),
                        z: hi.z.max(high.z),
                    },
                )
            }));
        }
        bounds
    }

    /// The grids of two geometries, in the order that merging places their primitives.
    pub(crate) fn merged_volumes(first: &Self, second: &Self) -> Option<Arc<[VolumeEntry]>> {
        let entries: Vec<VolumeEntry> = first
            .volumes
            .iter()
            .chain(second.volumes.iter())
            .flat_map(|entries| entries.iter().cloned())
            .collect();
        (!entries.is_empty()).then(|| Arc::from(entries))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AttrDomain, AttributeColumn, PackedPrototype};

    fn v(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    /// Active voxels at (1, 1, 1) and (10, 2, 3), a brick apart, with voxels 0.5 wide.
    fn grid() -> Result<Arc<VdbGrid>, GeoError> {
        Ok(Arc::new(
            VdbGrid::new(0.5, 2.0)?
                .with_voxel(1, 1, 1, -0.5)?
                .with_voxel(10, 2, 3, -0.25)?,
        ))
    }

    #[test]
    fn active_voxels_are_counted_and_bounded_exactly() -> Result<(), GeoError> {
        let two = grid()?;
        assert_eq!(two.active_voxel_count(), 2);
        // Each voxel fills a cell from its index to the next, so the far corner is index + 1.
        assert_eq!(
            two.active_bounds()?,
            Some((v(0.5, 0.5, 0.5), v(5.5, 1.5, 2.0)))
        );
        // Across negative coordinates, where a brick's voxels run the other way from zero.
        let negative = VdbGrid::new(1.0, 2.0)?.with_voxel(-1, -9, 16, -1.0)?;
        assert_eq!(
            negative.active_bounds()?,
            Some((v(-1.0, -9.0, 16.0), v(0.0, -8.0, 17.0)))
        );
        // Voxels at or above the background do not count, even in an allocated brick.
        let inactive = VdbGrid::new(1.0, 2.0)?.with_voxel(0, 0, 0, 2.0)?;
        assert_eq!(inactive.active_brick_count(), 1);
        assert_eq!(inactive.active_voxel_count(), 0);
        assert_eq!(inactive.active_bounds()?, None);
        assert_eq!(VdbGrid::new(1.0, 2.0)?.active_bounds()?, None);
        // Beyond what a finite position can express.
        let vast = VdbGrid::new(1e38, 2.0)?.with_voxel(1 << 20, 0, 0, -1.0)?;
        assert_eq!(vast.active_bounds().err(), Some(GeoError::NonFinite));
        assert_eq!(
            GeoDetail::from_volumes(vec![Arc::new(vast)]).err(),
            Some(GeoError::NonFinite)
        );
        Ok(())
    }

    #[test]
    fn a_volume_is_a_primitive_anchored_at_its_voxels() -> Result<(), GeoError> {
        let geo = GeoDetail::from_volumes(vec![grid()?])?;
        assert_eq!((geo.len(), geo.prim_count(), geo.volume_count()), (1, 1, 1));
        assert_eq!(geo.prim_runs()[0].kind, PrimKind::Volume);
        assert_eq!(geo.point(0), Some(v(3.0, 1.0, 1.25)));
        assert!(geo.volume(0).is_some_and(|g| g.active_voxel_count() == 2));
        assert!(geo.volume(1).is_none());
        // Reaches past its single point, as far as its voxels go.
        assert_eq!(geo.bounds(), Some((v(0.5, 0.5, 0.5), v(5.5, 1.5, 2.0))));
        // Nothing at all is not an error, and an empty grid still anchors at the origin.
        assert_eq!(GeoDetail::from_volumes(Vec::new())?.prim_count(), 0);
        let empty = GeoDetail::from_volumes(vec![Arc::new(VdbGrid::new(1.0, 2.0)?)])?;
        assert_eq!(empty.point(0), Some(Vec3::ZERO));
        assert_eq!(empty.bounds(), Some((Vec3::ZERO, Vec3::ZERO)));
        Ok(())
    }

    #[test]
    fn volumes_only_come_from_the_volume_api() -> Result<(), GeoError> {
        let run = |arity| PrimRun {
            kind: PrimKind::Volume,
            arity,
            prim_count: 1,
        };
        let cloud = GeoDetail::points(vec![Vec3::ZERO; 2]);
        // A volume primitive needs a grid, which plain topology cannot supply.
        assert_eq!(
            cloud.with_primitives(vec![run(1)], vec![0]).err(),
            Some(GeoError::InvalidTopology)
        );
        assert_eq!(
            cloud.with_runs(vec![run(2)], vec![0, 1]).err(),
            Some(GeoError::InvalidTopology)
        );
        // A file's grids must match its primitives exactly.
        let shell = cloud.with_runs(vec![run(1)], vec![0])?;
        assert_eq!(
            shell.with_volume_grids(Vec::new()).err(),
            Some(GeoError::InvalidTopology)
        );
        assert_eq!(
            shell.with_volume_grids(vec![grid()?, grid()?]).err(),
            Some(GeoError::InvalidTopology)
        );
        let filled = shell.with_volume_grids(vec![grid()?])?;
        assert_eq!(filled.volume_count(), 1);
        // Replacing topology removes the grids along with the primitives that held them.
        assert_eq!(
            filled
                .with_primitives(Vec::new(), Vec::new())?
                .volume_count(),
            0
        );
        Ok(())
    }

    #[test]
    fn replacing_grids_keeps_points_and_attributes() -> Result<(), GeoError> {
        let geo = GeoDetail::from_volumes(vec![grid()?])?.with_domain_attribute(
            AttrDomain::Prim,
            "tag",
            AttributeColumn::i32_values(vec![7]),
        )?;
        let other = Arc::new(VdbGrid::new(1.0, 2.0)?.with_voxel(0, 0, 0, -1.0)?);
        let next = geo.with_volume_grids(vec![Arc::clone(&other)])?;
        assert!(next.volume(0).is_some_and(|g| Arc::ptr_eq(g, &other)));
        assert_eq!(next.point(0), geo.point(0));
        assert!(next.domain_attribute(AttrDomain::Prim, "tag").is_some());
        // Bounds follow the grid held now, not the one held before.
        assert_eq!(next.volume_bounds(), Some((Vec3::ZERO, v(1.0, 1.0, 1.0))));
        Ok(())
    }

    #[test]
    fn hashes_follow_the_grids() -> Result<(), GeoError> {
        let geo = GeoDetail::from_volumes(vec![grid()?])?;
        assert_eq!(geo.hash(), GeoDetail::from_volumes(vec![grid()?])?.hash());
        // Same anchor and the same topology: only the grid differs.
        let changed = (*grid()?).with_voxel(1, 1, 1, -0.75)?;
        let other = geo.with_volume_grids(vec![Arc::new(changed)])?;
        assert_eq!(other.point(0), geo.point(0));
        assert_ne!(geo.hash(), other.hash());
        // A volume is not a packed instance or a point with the same shape.
        let plain = GeoDetail::points(vec![v(3.0, 1.0, 1.25)]);
        assert_ne!(geo.hash(), plain.hash());
        Ok(())
    }

    #[test]
    fn merging_keeps_each_grid_with_its_primitive() -> Result<(), GeoError> {
        let first = GeoDetail::from_volumes(vec![grid()?])?;
        let far = Arc::new(VdbGrid::new(1.0, 2.0)?.with_voxel(100, 0, 0, -1.0)?);
        let second = GeoDetail::from_volumes(vec![Arc::clone(&far)])?;
        let merged = first.merged(&second, usize::MAX)?;
        assert_eq!((merged.volume_count(), merged.prim_count()), (2, 2));
        assert_eq!(merged.prim_runs().len(), 1);
        assert!(merged.volume(1).is_some_and(|g| Arc::ptr_eq(g, &far)));
        // Both grids set the bounds, including the second's far side.
        assert_eq!(
            merged.bounds(),
            Some((v(0.5, 0.0, 0.0), v(101.0, 1.5, 2.0)))
        );
        // Volumes beside surfaces keep their place among the primitives.
        let triangle = GeoDetail::points(vec![Vec3::ZERO, v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)])
            .with_primitives(
                vec![PrimRun {
                    kind: PrimKind::Polygon,
                    arity: 3,
                    prim_count: 1,
                }],
                vec![0, 1, 2],
            )?;
        let mixed = triangle.merged(&first, usize::MAX)?;
        assert_eq!((mixed.volume_count(), mixed.prim_count()), (1, 2));
        // And merging without any leaves no table behind.
        assert_eq!(triangle.merged(&triangle, usize::MAX)?.volume_count(), 0);
        Ok(())
    }

    #[test]
    fn volumes_survive_unpacking_and_cannot_be_instanced_yet() -> Result<(), GeoError> {
        let geo = GeoDetail::from_volumes(vec![grid()?])?;
        assert_eq!(
            PackedPrototype::new(Arc::new(geo.clone())).err(),
            Some(GeoError::InvalidTopology)
        );
        // Instances beside a volume: unpacking them must not lose the volume.
        let triangle = GeoDetail::points(vec![Vec3::ZERO, v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)])
            .with_primitives(
                vec![PrimRun {
                    kind: PrimKind::Polygon,
                    arity: 3,
                    prim_count: 1,
                }],
                vec![0, 1, 2],
            )?;
        let packed = GeoDetail::packed_whole(Arc::new(triangle))?;
        let flat = geo
            .merged(&packed, usize::MAX)?
            .unpacked_cancellable(1 << 20, &std::sync::atomic::AtomicBool::new(false))?;
        assert_eq!(flat.packed_count(), 0);
        assert_eq!(flat.volume_count(), 1);
        // Winding means nothing to a volume, so reversal leaves it alone.
        assert_eq!(geo.with_reversed_winding()?.volume_count(), 1);
        Ok(())
    }
}
