//! 2D Copy-on-Write tiled heightfield terrain layers, derivative-warped multi-fractal fBm noise,
//! shallow-water virtual-pipe hydraulic + thermal erosion, and `GeoDetail` mesh conversion.

use pcc_geo::{
    AttrDomain, AttributeColumn, FiniteF32, GeoDetail, GeoError, PAGE_LEN, PrimKind, PrimRun,
};
use pcc_math::{Philox4x32, Vec3};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Logical 2D tile dimension ($64 \times 64 = 4,096$ samples per CoW tile).
pub const TILE_DIM: usize = 64;

#[derive(Clone, Debug)]
pub struct Tile2D {
    values: Arc<[FiniteF32]>,
    hash: blake3::Hash,
}

impl Tile2D {
    #[must_use]
    pub fn from_vec(values: Vec<FiniteF32>) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pcc-terrain-tile-v1");
        hasher.update(&(values.len() as u64).to_le_bytes());
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
}

#[derive(Clone, Debug)]
pub struct TiledLayer {
    rows: usize,
    cols: usize,
    tiles_r: usize,
    tiles_c: usize,
    tiles: Arc<[Tile2D]>,
}

impl TiledLayer {
    pub fn from_row_major(rows: usize, cols: usize, values: &[f32]) -> Result<Self, GeoError> {
        let expected = rows.checked_mul(cols).ok_or(GeoError::LimitExceeded)?;
        if rows < 2 || cols < 2 {
            return Err(GeoError::InvalidTopology);
        }
        if values.len() != expected {
            return Err(GeoError::AttributeLengthMismatch {
                expected,
                actual: values.len(),
            });
        }
        let tiles_r = rows.div_ceil(TILE_DIM);
        let tiles_c = cols.div_ceil(TILE_DIM);
        let tile_count = tiles_r
            .checked_mul(tiles_c)
            .ok_or(GeoError::LimitExceeded)?;
        let mut tiles = Vec::with_capacity(tile_count);
        for tr in 0..tiles_r {
            let r_start = tr * TILE_DIM;
            let r_end = (r_start + TILE_DIM).min(rows);
            for tc in 0..tiles_c {
                let c_start = tc * TILE_DIM;
                let c_end = (c_start + TILE_DIM).min(cols);
                let mut tile_vals = Vec::with_capacity((r_end - r_start) * (c_end - c_start));
                for r in r_start..r_end {
                    let row_offset = r * cols;
                    for c in c_start..c_end {
                        tile_vals.push(FiniteF32::checked(values[row_offset + c])?);
                    }
                }
                tiles.push(Tile2D::from_vec(tile_vals));
            }
        }
        Ok(Self {
            rows,
            cols,
            tiles_r,
            tiles_c,
            tiles: Arc::from(tiles),
        })
    }

    pub fn constant(rows: usize, cols: usize, value: f32) -> Result<Self, GeoError> {
        let checked = FiniteF32::checked(value)?;
        let total = rows.checked_mul(cols).ok_or(GeoError::LimitExceeded)?;
        let values = vec![checked.get(); total];
        Self::from_row_major(rows, cols, &values)
    }

    #[must_use]
    pub const fn rows(&self) -> usize {
        self.rows
    }

    #[must_use]
    pub const fn cols(&self) -> usize {
        self.cols
    }

    #[must_use]
    pub const fn tiles_r(&self) -> usize {
        self.tiles_r
    }

    #[must_use]
    pub const fn tiles_c(&self) -> usize {
        self.tiles_c
    }

    #[must_use]
    pub fn get(&self, r: usize, c: usize) -> Option<FiniteF32> {
        if r >= self.rows || c >= self.cols {
            return None;
        }
        let tr = r / TILE_DIM;
        let tc = c / TILE_DIM;
        let tile = self.tiles.get(tr * self.tiles_c + tc)?;
        let r_start = tr * TILE_DIM;
        let c_start = tc * TILE_DIM;
        let tile_cols = (c_start + TILE_DIM).min(self.cols) - c_start;
        let local_idx = (r - r_start) * tile_cols + (c - c_start);
        tile.values.get(local_idx).copied()
    }

    /// Updates a single sample `(r, c)`, copying only the affected $64 \times 64$ tile
    /// while sharing all other tiles via `Arc`.
    pub fn with_sample(&self, r: usize, c: usize, value: f32) -> Result<Self, GeoError> {
        if r >= self.rows || c >= self.cols {
            return Err(GeoError::IndexOutOfBounds);
        }
        let checked = FiniteF32::checked(value)?;
        let tr = r / TILE_DIM;
        let tc = c / TILE_DIM;
        let tile_idx = tr * self.tiles_c + tc;
        let r_start = tr * TILE_DIM;
        let c_start = tc * TILE_DIM;
        let tile_cols = (c_start + TILE_DIM).min(self.cols) - c_start;
        let local_idx = (r - r_start) * tile_cols + (c - c_start);

        let mut next_tiles = self.tiles.to_vec();
        let mut tile_vals = next_tiles[tile_idx].values.to_vec();
        if let Some(slot) = tile_vals.get_mut(local_idx) {
            *slot = checked;
        } else {
            return Err(GeoError::IndexOutOfBounds);
        }
        next_tiles[tile_idx] = Tile2D::from_vec(tile_vals);
        Ok(Self {
            rows: self.rows,
            cols: self.cols,
            tiles_r: self.tiles_r,
            tiles_c: self.tiles_c,
            tiles: Arc::from(next_tiles),
        })
    }

    #[must_use]
    pub fn to_row_major_vec(&self) -> Vec<f32> {
        let mut out = vec![0.0f32; self.rows * self.cols];
        for r in 0..self.rows {
            for c in 0..self.cols {
                if let Some(v) = self.get(r, c) {
                    out[r * self.cols + c] = v.get();
                }
            }
        }
        out
    }

    #[must_use]
    pub fn tile_shared_with(&self, other: &Self, tr: usize, tc: usize) -> bool {
        if self.tiles_c != other.tiles_c {
            return false;
        }
        let idx = tr * self.tiles_c + tc;
        self.tiles.get(idx).is_some_and(|a| {
            other
                .tiles
                .get(idx)
                .is_some_and(|b| Arc::ptr_eq(&a.values, &b.values))
        })
    }

    #[must_use]
    pub fn hash(&self) -> blake3::Hash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pcc-terrain-layer-v1");
        hasher.update(&(self.rows as u64).to_le_bytes());
        hasher.update(&(self.cols as u64).to_le_bytes());
        for tile in &*self.tiles {
            hasher.update(tile.hash.as_bytes());
        }
        hasher.finalize()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HfNoiseParams {
    pub amplitude: f32,
    pub frequency: f32,
    pub octaves: u32,
    pub lacunarity: f32,
    pub gain: f32,
    pub seed: u64,
}

impl Default for HfNoiseParams {
    fn default() -> Self {
        Self {
            amplitude: 5.0,
            frequency: 0.1,
            octaves: 4,
            lacunarity: 2.0,
            gain: 0.5,
            seed: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HfErodeParams {
    pub iterations: u32,
    pub dt: f32,
    pub rain_rate: f32,
    pub evaporation: f32,
    pub solubility: f32,
    pub deposition: f32,
    pub capacity: f32,
    pub talus_angle_deg: f32,
    pub thermal_rate: f32,
    pub seed: u64,
}

impl Default for HfErodeParams {
    fn default() -> Self {
        Self {
            iterations: 10,
            dt: 0.1,
            rain_rate: 0.05,
            evaporation: 0.05,
            solubility: 0.3,
            deposition: 0.3,
            capacity: 4.0,
            talus_angle_deg: 35.0,
            thermal_rate: 0.15,
            seed: 1,
        }
    }
}

/// Multi-layer 2D Copy-on-Write tiled heightfield terrain.
#[derive(Clone, Debug)]
pub struct HeightField {
    rows: usize,
    cols: usize,
    size_x: f32,
    size_z: f32,
    layers: Arc<BTreeMap<String, TiledLayer>>,
}

impl HeightField {
    pub fn flat(
        rows: usize,
        cols: usize,
        size_x: f32,
        size_z: f32,
        initial_height: f32,
    ) -> Result<Self, GeoError> {
        if rows < 2 || cols < 2 {
            return Err(GeoError::InvalidTopology);
        }
        if !size_x.is_finite() || !size_z.is_finite() || !initial_height.is_finite() {
            return Err(GeoError::NonFinite);
        }
        if size_x <= 0.0 || size_z <= 0.0 {
            return Err(GeoError::InvalidTopology);
        }
        let height_layer = TiledLayer::constant(rows, cols, initial_height)?;
        let mask_layer = TiledLayer::constant(rows, cols, 1.0)?;
        let zero_layer = TiledLayer::constant(rows, cols, 0.0)?;
        let mut layers = BTreeMap::new();
        layers.insert("height".to_owned(), height_layer);
        layers.insert("mask".to_owned(), mask_layer);
        layers.insert("water".to_owned(), zero_layer.clone());
        layers.insert("sediment".to_owned(), zero_layer);
        Ok(Self {
            rows,
            cols,
            size_x: if size_x == 0.0 { 0.0 } else { size_x },
            size_z: if size_z == 0.0 { 0.0 } else { size_z },
            layers: Arc::new(layers),
        })
    }

    #[must_use]
    pub const fn rows(&self) -> usize {
        self.rows
    }

    #[must_use]
    pub const fn cols(&self) -> usize {
        self.cols
    }

    #[must_use]
    pub const fn size_x(&self) -> f32 {
        self.size_x
    }

    #[must_use]
    pub const fn size_z(&self) -> f32 {
        self.size_z
    }

    #[must_use]
    pub fn layer(&self, name: &str) -> Option<&TiledLayer> {
        self.layers.get(name)
    }

    #[must_use]
    pub fn layer_names(&self) -> Vec<&str> {
        self.layers.keys().map(String::as_str).collect()
    }

    pub fn with_layer(&self, name: impl Into<String>, layer: TiledLayer) -> Result<Self, GeoError> {
        let name = name.into();
        if layer.rows() != self.rows || layer.cols() != self.cols {
            return Err(GeoError::AttributeLengthMismatch {
                expected: self.rows * self.cols,
                actual: layer.rows() * layer.cols(),
            });
        }
        let mut next = self.clone();
        let mut map = (*self.layers).clone();
        map.insert(name, layer);
        next.layers = Arc::new(map);
        Ok(next)
    }

    #[must_use]
    pub fn sample(&self, layer_name: &str, r: usize, c: usize) -> Option<f32> {
        self.layers
            .get(layer_name)
            .and_then(|l| l.get(r, c))
            .map(FiniteF32::get)
    }

    #[must_use]
    pub fn hash(&self) -> blake3::Hash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pcc-terrain-hf-v1");
        hasher.update(&(self.rows as u64).to_le_bytes());
        hasher.update(&(self.cols as u64).to_le_bytes());
        hasher.update(&self.size_x.to_bits().to_le_bytes());
        hasher.update(&self.size_z.to_bits().to_le_bytes());
        hasher.update(&(self.layers.len() as u64).to_le_bytes());
        for (name, layer) in &*self.layers {
            hasher.update(&(name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
            hasher.update(layer.hash().as_bytes());
        }
        hasher.finalize()
    }

    /// Evaluates deterministic derivative-warped multi-fractal fBm noise over the `height` layer
    /// (modulated by `mask` if present), preserving Copy-on-Write tile sharing for all untouched layers.
    pub fn apply_fbm_noise_cancellable(
        &self,
        params: &HfNoiseParams,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        if !params.amplitude.is_finite()
            || !params.frequency.is_finite()
            || !params.lacunarity.is_finite()
            || !params.gain.is_finite()
        {
            return Err(GeoError::NonFinite);
        }
        if params.octaves == 0 || params.octaves > 12 || params.frequency <= 0.0 {
            return Err(GeoError::InvalidTopology);
        }
        let total = self
            .rows
            .checked_mul(self.cols)
            .ok_or(GeoError::LimitExceeded)?;
        let bytes = total
            .checked_mul(std::mem::size_of::<f32>())
            .ok_or(GeoError::LimitExceeded)?;
        if bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }
        let height_layer = self.layer("height").ok_or(GeoError::AttributeNotFound)?;
        let mask_layer = self.layer("mask");
        let denom_r = (self.rows - 1) as f32;
        let denom_c = (self.cols - 1) as f32;

        let mut new_heights = Vec::with_capacity(total);
        for r in 0..self.rows {
            for c in 0..self.cols {
                let idx = new_heights.len();
                if idx.is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                    return Err(GeoError::Cancelled);
                }
                let x = (c as f32 / denom_c) * self.size_x;
                let z = (r as f32 / denom_r) * self.size_z;
                let base_h = height_layer.get(r, c).map(FiniteF32::get).unwrap_or(0.0);
                let mask_val = mask_layer
                    .and_then(|m| m.get(r, c))
                    .map(|v| v.get().clamp(0.0, 1.0))
                    .unwrap_or(1.0);
                let fbm = eval_derivative_warped_fbm(x, z, params);
                let h = base_h + mask_val * params.amplitude * fbm;
                if !h.is_finite() {
                    return Err(GeoError::NonFinite);
                }
                new_heights.push(h);
            }
        }
        let updated_height = TiledLayer::from_row_major(self.rows, self.cols, &new_heights)?;
        self.with_layer("height", updated_height)
    }

    /// Runs shallow-water virtual-pipe hydraulic erosion + thermal talus slumping
    /// (Mei, Decaudin & Hu 2007; Št'ava et al. 2008), updating `height`, `water`, and `sediment` layers.
    pub fn erode_cancellable(
        &self,
        params: &HfErodeParams,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        if !params.dt.is_finite()
            || !params.rain_rate.is_finite()
            || !params.evaporation.is_finite()
            || !params.solubility.is_finite()
            || !params.deposition.is_finite()
            || !params.capacity.is_finite()
            || !params.talus_angle_deg.is_finite()
            || !params.thermal_rate.is_finite()
        {
            return Err(GeoError::NonFinite);
        }
        if params.iterations == 0
            || params.iterations > 500
            || params.dt <= 0.0
            || params.rain_rate < 0.0
            || !(1.0..=89.0).contains(&params.talus_angle_deg)
        {
            return Err(GeoError::InvalidTopology);
        }
        let total = self
            .rows
            .checked_mul(self.cols)
            .ok_or(GeoError::LimitExceeded)?;
        // Working buffers: h, w, s, flux(4), vel(2), h_next, s_next = 11 f32 per cell
        let bytes = total
            .checked_mul(11 * std::mem::size_of::<f32>())
            .ok_or(GeoError::LimitExceeded)?;
        if bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }

        let mut h = self
            .layer("height")
            .ok_or(GeoError::AttributeNotFound)?
            .to_row_major_vec();
        let mut w = self
            .layer("water")
            .map_or_else(|| vec![0.0f32; total], TiledLayer::to_row_major_vec);
        let mut s = self
            .layer("sediment")
            .map_or_else(|| vec![0.0f32; total], TiledLayer::to_row_major_vec);

        let mut flux_l = vec![0.0f32; total];
        let mut flux_r = vec![0.0f32; total];
        let mut flux_u = vec![0.0f32; total];
        let mut flux_d = vec![0.0f32; total];
        let mut vel_x = vec![0.0f32; total];
        let mut vel_z = vec![0.0f32; total];

        let dx = (self.size_x / ((self.cols - 1) as f32)).max(1e-3);
        let dz = (self.size_z / ((self.rows - 1) as f32)).max(1e-3);
        let cell_area = dx * dz;
        let gravity = 9.81f32;
        let talus_tan = params.talus_angle_deg.to_radians().tan();
        let rng = Philox4x32::new(params.seed);

        for iter in 0..params.iterations {
            if cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            // 1. Rainfall with slight deterministic spatial modulation
            for r in 0..self.rows {
                for c in 0..self.cols {
                    let idx = r * self.cols + c;
                    let jitter = rng.generate_unit_f32([iter, r as u32, c as u32, 0])[0];
                    let rain = params.rain_rate * params.dt * (0.85 + 0.30 * jitter);
                    w[idx] = (w[idx] + rain).max(0.0);
                }
            }

            // 2. Virtual pipe hydrostatic outflow flux update
            for r in 0..self.rows {
                for c in 0..self.cols {
                    let idx = r * self.cols + c;
                    let hw = h[idx] + w[idx];

                    let dh_l = if c > 0 {
                        hw - (h[idx - 1] + w[idx - 1])
                    } else {
                        0.0
                    };
                    let dh_r = if c + 1 < self.cols {
                        hw - (h[idx + 1] + w[idx + 1])
                    } else {
                        0.0
                    };
                    let dh_u = if r > 0 {
                        hw - (h[idx - self.cols] + w[idx - self.cols])
                    } else {
                        0.0
                    };
                    let dh_d = if r + 1 < self.rows {
                        hw - (h[idx + self.cols] + w[idx + self.cols])
                    } else {
                        0.0
                    };

                    let fl = (flux_l[idx] + params.dt * gravity * dh_l / dx).max(0.0);
                    let fr = (flux_r[idx] + params.dt * gravity * dh_r / dx).max(0.0);
                    let fu = (flux_u[idx] + params.dt * gravity * dh_u / dz).max(0.0);
                    let fd = (flux_d[idx] + params.dt * gravity * dh_d / dz).max(0.0);

                    let sum_f = fl + fr + fu + fd;
                    let scale = if sum_f * params.dt > w[idx] * cell_area && sum_f > 1e-8 {
                        (w[idx] * cell_area) / (sum_f * params.dt)
                    } else {
                        1.0
                    };
                    flux_l[idx] = fl * scale;
                    flux_r[idx] = fr * scale;
                    flux_u[idx] = fu * scale;
                    flux_d[idx] = fd * scale;
                }
            }

            // 3. Water depth & velocity field update
            for r in 0..self.rows {
                for c in 0..self.cols {
                    let idx = r * self.cols + c;
                    let in_l = if c > 0 { flux_r[idx - 1] } else { 0.0 };
                    let in_r = if c + 1 < self.cols {
                        flux_l[idx + 1]
                    } else {
                        0.0
                    };
                    let in_u = if r > 0 { flux_d[idx - self.cols] } else { 0.0 };
                    let in_d = if r + 1 < self.rows {
                        flux_u[idx + self.cols]
                    } else {
                        0.0
                    };
                    let inflow = in_l + in_r + in_u + in_d;
                    let outflow = flux_l[idx] + flux_r[idx] + flux_u[idx] + flux_d[idx];
                    let dv = params.dt * (inflow - outflow);
                    let w_next = (w[idx] + dv / cell_area).max(0.0);
                    w[idx] = w_next;

                    let wx = 0.5 * (in_l - flux_l[idx] + flux_r[idx] - in_r);
                    let wz = 0.5 * (in_u - flux_u[idx] + flux_d[idx] - in_d);
                    vel_x[idx] = wx / dx;
                    vel_z[idx] = wz / dz;
                }
            }

            // 4. Hydraulic erosion & deposition
            for r in 0..self.rows {
                for c in 0..self.cols {
                    let idx = r * self.cols + c;
                    let hl = if c > 0 { h[idx - 1] } else { h[idx] };
                    let hr = if c + 1 < self.cols {
                        h[idx + 1]
                    } else {
                        h[idx]
                    };
                    let hu = if r > 0 { h[idx - self.cols] } else { h[idx] };
                    let hd = if r + 1 < self.rows {
                        h[idx + self.cols]
                    } else {
                        h[idx]
                    };
                    let dh_dx = (hr - hl) / (2.0 * dx);
                    let dh_dz = (hd - hu) / (2.0 * dz);
                    let grad_sq = dh_dx * dh_dx + dh_dz * dh_dz;
                    let sin_alpha = (grad_sq.sqrt() / (1.0 + grad_sq).sqrt()).max(0.05);
                    let speed = (vel_x[idx] * vel_x[idx] + vel_z[idx] * vel_z[idx]).sqrt();
                    let cap = params.capacity * sin_alpha * speed;

                    if cap > s[idx] {
                        let delta = ((cap - s[idx]) * params.solubility * params.dt).min(0.5);
                        h[idx] -= delta;
                        s[idx] += delta;
                    } else {
                        let delta = ((s[idx] - cap) * params.deposition * params.dt).min(s[idx]);
                        h[idx] += delta;
                        s[idx] -= delta;
                    }
                }
            }

            // 5. Thermal talus slumping (relaxes slopes exceeding talus_angle_deg)
            let h_snap = h.clone();
            for r in 0..self.rows {
                for c in 0..self.cols {
                    let idx = r * self.cols + c;
                    let hc = h_snap[idx];
                    let neighbors: [(Option<usize>, f32); 4] = [
                        (c.checked_sub(1).map(|cc| r * self.cols + cc), dx),
                        (
                            if c + 1 < self.cols {
                                Some(r * self.cols + c + 1)
                            } else {
                                None
                            },
                            dx,
                        ),
                        (r.checked_sub(1).map(|rr| rr * self.cols + c), dz),
                        (
                            if r + 1 < self.rows {
                                Some((r + 1) * self.cols + c)
                            } else {
                                None
                            },
                            dz,
                        ),
                    ];
                    for (opt_n, dist) in neighbors {
                        if let Some(n_idx) = opt_n {
                            let diff = hc - h_snap[n_idx];
                            let max_diff = talus_tan * dist;
                            if diff > max_diff {
                                let transfer = 0.125 * params.thermal_rate * (diff - max_diff);
                                h[idx] -= transfer;
                                h[n_idx] += transfer;
                            }
                        }
                    }
                }
            }

            // 6. Semi-Lagrangian sediment advection & water evaporation
            let s_snap = s.clone();
            for r in 0..self.rows {
                for c in 0..self.cols {
                    let idx = r * self.cols + c;
                    let src_c = ((c as f32) - (vel_x[idx] * params.dt / dx))
                        .clamp(0.0, (self.cols - 1) as f32);
                    let src_r = ((r as f32) - (vel_z[idx] * params.dt / dz))
                        .clamp(0.0, (self.rows - 1) as f32);
                    let c0 = src_c.floor() as usize;
                    let r0 = src_r.floor() as usize;
                    let c1 = (c0 + 1).min(self.cols - 1);
                    let r1 = (r0 + 1).min(self.rows - 1);
                    let fc = src_c - (c0 as f32);
                    let fr = src_r - (r0 as f32);
                    let s00 = s_snap[r0 * self.cols + c0];
                    let s10 = s_snap[r0 * self.cols + c1];
                    let s01 = s_snap[r1 * self.cols + c0];
                    let s11 = s_snap[r1 * self.cols + c1];
                    let top = s00 * (1.0 - fc) + s10 * fc;
                    let bot = s01 * (1.0 - fc) + s11 * fc;
                    s[idx] = (top * (1.0 - fr) + bot * fr).max(0.0);
                    w[idx] = (w[idx] * (1.0 - params.evaporation * params.dt)).max(0.0);
                }
            }
        }

        let next_h = TiledLayer::from_row_major(self.rows, self.cols, &h)?;
        let next_w = TiledLayer::from_row_major(self.rows, self.cols, &w)?;
        let next_s = TiledLayer::from_row_major(self.rows, self.cols, &s)?;
        self.with_layer("height", next_h)?
            .with_layer("water", next_w)?
            .with_layer("sediment", next_s)
    }

    /// Converts this `HeightField` into a quad-mesh `GeoDetail` with `@P.y = height(r, c)`,
    /// attaching all 2D layers as `AttrDomain::Point` `F32` columns and grid dimensions on `AttrDomain::Detail`.
    pub fn to_geo_detail_cancellable(
        &self,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<GeoDetail, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        let point_count = self
            .rows
            .checked_mul(self.cols)
            .ok_or(GeoError::LimitExceeded)?;
        let quad_count = (self.rows - 1)
            .checked_mul(self.cols - 1)
            .ok_or(GeoError::LimitExceeded)?;
        let vtx_count = quad_count.checked_mul(4).ok_or(GeoError::LimitExceeded)?;
        let pt_bytes = point_count
            .checked_mul(std::mem::size_of::<Vec3>())
            .ok_or(GeoError::LimitExceeded)?;
        let vtx_bytes = vtx_count
            .checked_mul(std::mem::size_of::<u32>())
            .ok_or(GeoError::LimitExceeded)?;
        if pt_bytes.saturating_add(vtx_bytes) > memory_limit {
            return Err(GeoError::LimitExceeded);
        }

        let height_layer = self.layer("height").ok_or(GeoError::AttributeNotFound)?;
        let denom_r = (self.rows - 1) as f32;
        let denom_c = (self.cols - 1) as f32;
        let mut points = Vec::with_capacity(point_count);
        for r in 0..self.rows {
            for c in 0..self.cols {
                if points.len().is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                    return Err(GeoError::Cancelled);
                }
                let x = (c as f32 / denom_c) * self.size_x;
                let z = (r as f32 / denom_r) * self.size_z;
                let y = height_layer.get(r, c).map(FiniteF32::get).unwrap_or(0.0);
                points.push(Vec3::checked(x, y, z)?);
            }
        }

        let mut vertex_points = Vec::with_capacity(vtx_count);
        let cols_u32 = u32::try_from(self.cols).map_err(|_| GeoError::LimitExceeded)?;
        for r in 0..(self.rows as u32 - 1) {
            for c in 0..(cols_u32 - 1) {
                let i0 = r * cols_u32 + c;
                let i1 = i0 + 1;
                let i2 = i0 + cols_u32 + 1;
                let i3 = i0 + cols_u32;
                vertex_points.extend_from_slice(&[i0, i1, i2, i3]);
            }
        }

        let mut geo = GeoDetail::points(points).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 4,
                prim_count: quad_count,
            }],
            vertex_points,
        )?;

        for (name, layer) in &*self.layers {
            let col = AttributeColumn::f32_checked(layer.to_row_major_vec())?;
            geo = geo.with_domain_attribute(AttrDomain::Point, name.clone(), col)?;
        }
        let rows_i32 = i32::try_from(self.rows).map_err(|_| GeoError::LimitExceeded)?;
        let cols_i32 = i32::try_from(self.cols).map_err(|_| GeoError::LimitExceeded)?;
        geo = geo
            .with_domain_attribute(
                AttrDomain::Detail,
                "hf_rows",
                AttributeColumn::i32_values(vec![rows_i32]),
            )?
            .with_domain_attribute(
                AttrDomain::Detail,
                "hf_cols",
                AttributeColumn::i32_values(vec![cols_i32]),
            )?
            .with_domain_attribute(
                AttrDomain::Detail,
                "hf_size_x",
                AttributeColumn::f32_checked(vec![self.size_x])?,
            )?
            .with_domain_attribute(
                AttrDomain::Detail,
                "hf_size_z",
                AttributeColumn::f32_checked(vec![self.size_z])?,
            )?;
        Ok(geo)
    }

    /// Reconstructs a `HeightField` from a `GeoDetail` produced by `HeightField` or `CreateGrid`.
    pub fn from_geo_detail_cancellable(
        geo: &GeoDetail,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        let pt_count = geo.len();
        if pt_count < 4 {
            return Err(GeoError::InvalidTopology);
        }
        let bytes = pt_count
            .checked_mul(std::mem::size_of::<f32>())
            .ok_or(GeoError::LimitExceeded)?;
        if bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }

        let (rows, cols) = if let (Some(r), Some(c)) = (
            geo.domain_attribute(AttrDomain::Detail, "hf_rows")
                .and_then(AttributeColumn::as_i32)
                .and_then(|col| col.get(0)),
            geo.domain_attribute(AttrDomain::Detail, "hf_cols")
                .and_then(AttributeColumn::as_i32)
                .and_then(|col| col.get(0)),
        ) {
            let r_usize = usize::try_from(r).map_err(|_| GeoError::InvalidTopology)?;
            let c_usize = usize::try_from(c).map_err(|_| GeoError::InvalidTopology)?;
            (r_usize, c_usize)
        } else {
            // Infer cols from the first quad's fourth vertex index (`i3 = cols`)
            let inferred_cols = if geo.vertex_count() >= 4 {
                geo.vertex_point(3)
                    .map(|idx| idx as usize)
                    .unwrap_or(0)
                    .max(2)
            } else {
                0
            };
            if inferred_cols >= 2 && pt_count.is_multiple_of(inferred_cols) {
                (pt_count / inferred_cols, inferred_cols)
            } else {
                return Err(GeoError::InvalidTopology);
            }
        };

        if rows.checked_mul(cols) != Some(pt_count) {
            return Err(GeoError::InvalidTopology);
        }

        let (bmin, bmax) = geo.bounds().ok_or(GeoError::InvalidTopology)?;
        let size_x = geo
            .domain_attribute(AttrDomain::Detail, "hf_size_x")
            .and_then(AttributeColumn::as_f32)
            .and_then(|col| col.get(0))
            .map(FiniteF32::get)
            .unwrap_or_else(|| (bmax.x - bmin.x).max(1.0));
        let size_z = geo
            .domain_attribute(AttrDomain::Detail, "hf_size_z")
            .and_then(AttributeColumn::as_f32)
            .and_then(|col| col.get(0))
            .map(FiniteF32::get)
            .unwrap_or_else(|| (bmax.z - bmin.z).max(1.0));

        let mut heights = Vec::with_capacity(pt_count);
        for i in 0..pt_count {
            if i.is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            let p = geo.point(i).ok_or(GeoError::IndexOutOfBounds)?;
            heights.push(p.y);
        }

        let mut hf = Self::flat(rows, cols, size_x, size_z, 0.0)?;
        hf = hf.with_layer("height", TiledLayer::from_row_major(rows, cols, &heights)?)?;

        for layer_name in ["mask", "water", "sediment", "bedrock"] {
            if let Some(col) = geo.attribute(layer_name).and_then(AttributeColumn::as_f32) {
                let mut vals = Vec::with_capacity(pt_count);
                for i in 0..pt_count {
                    vals.push(col.get(i).map(FiniteF32::get).unwrap_or(0.0));
                }
                hf = hf.with_layer(layer_name, TiledLayer::from_row_major(rows, cols, &vals)?)?;
            }
        }
        Ok(hf)
    }
}

fn eval_2d_value_and_grad(x: f32, z: f32, octave: u32, rng: &Philox4x32) -> (f32, f32, f32) {
    let xi = x.floor();
    let zi = z.floor();
    let fx = x - xi;
    let fz = z - zi;
    let ix = xi as i32 as u32;
    let iz = zi as i32 as u32;

    // Quintic smoothstep u(t) = 6t^5 - 15t^4 + 10t^3, du/dt = 30t^4 - 60t^3 + 30t^2
    let ux = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let uz = fz * fz * fz * (fz * (fz * 6.0 - 15.0) + 10.0);
    let dux = 30.0 * fx * fx * (fx * (fx - 2.0) + 1.0);
    let duz = 30.0 * fz * fz * (fz * (fz - 2.0) + 1.0);

    let v00 = 2.0 * rng.generate_unit_f32([ix, iz, octave, 0])[0] - 1.0;
    let v10 = 2.0 * rng.generate_unit_f32([ix.wrapping_add(1), iz, octave, 0])[0] - 1.0;
    let v01 = 2.0 * rng.generate_unit_f32([ix, iz.wrapping_add(1), octave, 0])[0] - 1.0;
    let v11 =
        2.0 * rng.generate_unit_f32([ix.wrapping_add(1), iz.wrapping_add(1), octave, 0])[0] - 1.0;

    let k0 = v00;
    let k1 = v10 - v00;
    let k2 = v01 - v00;
    let k3 = v00 - v10 - v01 + v11;

    let val = k0 + k1 * ux + k2 * uz + k3 * ux * uz;
    let dx = (k1 + k3 * uz) * dux;
    let dz = (k2 + k3 * ux) * duz;
    (val, dx, dz)
}

fn eval_derivative_warped_fbm(x: f32, z: f32, params: &HfNoiseParams) -> f32 {
    let rng = Philox4x32::new(params.seed);
    let mut sum = 0.0f32;
    let mut weight = 1.0f32;
    let mut norm = 0.0f32;
    let mut freq = params.frequency;
    let mut dx_sum = 0.0f32;
    let mut dz_sum = 0.0f32;

    for oct in 0..params.octaves {
        let (val, gx, gz) = eval_2d_value_and_grad(x * freq, z * freq, oct, &rng);
        dx_sum += gx;
        dz_sum += gz;
        let slope_damping = 1.0 / (1.0 + dx_sum * dx_sum + dz_sum * dz_sum);
        sum += weight * val * slope_damping;
        norm += weight;
        weight *= params.gain;
        freq *= params.lacunarity;
    }
    if norm > 1e-6 { sum / norm } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiled_layer_cow_shares_unmodified_64x64_tiles_and_hashes() {
        let layer_res = TiledLayer::constant(128, 128, 1.0);
        assert!(layer_res.is_ok());
        let Ok(layer) = layer_res else {
            return;
        };
        assert_eq!(layer.tiles_r(), 2);
        assert_eq!(layer.tiles_c(), 2);

        // Mutate sample (5, 5) in tile (0, 0)
        let edited_res = layer.with_sample(5, 5, 9.5);
        assert!(edited_res.is_ok());
        if let Ok(edited) = edited_res {
            assert_eq!(edited.get(5, 5).map(FiniteF32::get), Some(9.5));
            assert_eq!(layer.get(5, 5).map(FiniteF32::get), Some(1.0));
            assert!(!edited.tile_shared_with(&layer, 0, 0));
            assert!(edited.tile_shared_with(&layer, 0, 1));
            assert!(edited.tile_shared_with(&layer, 1, 0));
            assert!(edited.tile_shared_with(&layer, 1, 1));
            assert_ne!(edited.hash(), layer.hash());
        }
    }

    #[test]
    fn heightfield_noise_and_shallow_water_erosion_round_trip_to_geo_detail() {
        let paused = AtomicBool::new(false);
        let hf_res = HeightField::flat(16, 16, 30.0, 30.0, 10.0);
        assert!(hf_res.is_ok());
        let Ok(hf0) = hf_res else {
            return;
        };

        let noise_params = HfNoiseParams {
            amplitude: 8.0,
            frequency: 0.08,
            octaves: 4,
            lacunarity: 2.0,
            gain: 0.5,
            seed: 42,
        };
        let hf_noisy_res = hf0.apply_fbm_noise_cancellable(&noise_params, 1 << 20, &paused);
        assert!(hf_noisy_res.is_ok());
        let Ok(hf_noisy) = hf_noisy_res else {
            return;
        };
        // Untouched `mask` layer shares its underlying tile Arc with hf0
        if let (Some(m0), Some(m1)) = (hf0.layer("mask"), hf_noisy.layer("mask")) {
            assert!(m1.tile_shared_with(m0, 0, 0));
        }

        let erode_params = HfErodeParams {
            iterations: 12,
            rain_rate: 0.08,
            talus_angle_deg: 32.0,
            seed: 7,
            ..HfErodeParams::default()
        };
        let hf_eroded_res = hf_noisy.erode_cancellable(&erode_params, 1 << 20, &paused);
        assert!(hf_eroded_res.is_ok());
        let Ok(hf_eroded) = hf_eroded_res else {
            return;
        };
        assert_ne!(hf_eroded.hash(), hf_noisy.hash());

        // Verify sediment and water layers have positive total mass after rainfall + erosion
        let total_water: f32 = hf_eroded
            .layer("water")
            .map_or_else(Vec::new, TiledLayer::to_row_major_vec)
            .into_iter()
            .sum();
        let total_sediment: f32 = hf_eroded
            .layer("sediment")
            .map_or_else(Vec::new, TiledLayer::to_row_major_vec)
            .into_iter()
            .sum();
        assert!(total_water > 0.0);
        assert!(total_sediment > 0.0);

        // Convert to GeoDetail and reconstruct back
        let geo_res = hf_eroded.to_geo_detail_cancellable(1 << 20, &paused);
        assert!(geo_res.is_ok());
        if let Ok(geo) = geo_res {
            assert_eq!(geo.len(), 256);
            assert_eq!(geo.prim_count(), 225);
            assert!(geo.topology_index().is_manifold());
            assert!(geo.attribute("height").is_some());
            assert!(geo.attribute("water").is_some());
            assert!(geo.attribute("sediment").is_some());

            let round_trip = HeightField::from_geo_detail_cancellable(&geo, 1 << 20, &paused);
            assert!(round_trip.is_ok());
            if let Ok(rt) = round_trip {
                assert_eq!(rt.hash(), hf_eroded.hash());
            }
        }
    }
}
