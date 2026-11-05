//! Storage for sparse $8^3$ Copy-on-Write brick volumes. It lives here so that geometry can
//! hold a grid; `pcc-vdb` holds the algorithms that work on one.

use crate::{FiniteF32, GeoError};
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

    /// Every allocated brick, keyed by brick coordinate.
    #[must_use]
    pub fn bricks(&self) -> &BTreeMap<(i32, i32, i32), Brick8x8x8> {
        &self.bricks
    }
}
