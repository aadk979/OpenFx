//! Shared helpers: budget and cancellation context, double-precision vectors, primitive views.

use pcc_geo::{GeoDetail, GeoError, PrimKind, PrimRun};
use pcc_math::Vec3;
use std::sync::atomic::{AtomicBool, Ordering};

/// Double-precision working coordinate. Algorithms compute in `f64` and round to `f32` on output.
pub(crate) type D3 = [f64; 3];

/// Cook-scoped limits passed to every operator.
#[derive(Clone, Copy)]
pub struct SopContext<'a> {
    pub memory_limit: usize,
    pub cancelled: &'a AtomicBool,
}

impl<'a> SopContext<'a> {
    #[must_use]
    pub const fn new(memory_limit: usize, cancelled: &'a AtomicBool) -> Self {
        Self {
            memory_limit,
            cancelled,
        }
    }

    pub(crate) fn poll(&self) -> Result<(), GeoError> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(GeoError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Polls cancellation once per 4,096 iterations of a hot loop.
    pub(crate) fn poll_every(&self, iteration: usize) -> Result<(), GeoError> {
        if iteration.is_multiple_of(4096) {
            self.poll()
        } else {
            Ok(())
        }
    }

    /// Fails when `points` positions and `vertices` indices would exceed the memory budget.
    pub(crate) fn check_output(&self, points: usize, vertices: usize) -> Result<(), GeoError> {
        let point_bytes = points
            .checked_mul(std::mem::size_of::<Vec3>())
            .ok_or(GeoError::LimitExceeded)?;
        let vertex_bytes = vertices
            .checked_mul(std::mem::size_of::<u32>())
            .ok_or(GeoError::LimitExceeded)?;
        let total = point_bytes
            .checked_add(vertex_bytes)
            .ok_or(GeoError::LimitExceeded)?;
        if total > self.memory_limit {
            Err(GeoError::LimitExceeded)
        } else {
            Ok(())
        }
    }
}

pub(crate) fn to_d3(v: Vec3) -> D3 {
    [f64::from(v.x), f64::from(v.y), f64::from(v.z)]
}

pub(crate) fn to_vec3(p: D3) -> Result<Vec3, GeoError> {
    Ok(Vec3::checked(p[0] as f32, p[1] as f32, p[2] as f32)?)
}

pub(crate) fn add(a: D3, b: D3) -> D3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn sub(a: D3, b: D3) -> D3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn scale(a: D3, s: f64) -> D3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub(crate) fn dot(a: D3, b: D3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn cross(a: D3, b: D3) -> D3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn length(a: D3) -> f64 {
    dot(a, a).sqrt()
}

pub(crate) fn distance(a: D3, b: D3) -> f64 {
    length(sub(a, b))
}

pub(crate) fn lerp(a: D3, b: D3, t: f64) -> D3 {
    add(a, scale(sub(b, a), t))
}

/// Unit vector along `a`, or `None` for a vector too short to normalize reliably.
pub(crate) fn normalized(a: D3) -> Option<D3> {
    let len = length(a);
    (len > 1e-30).then(|| scale(a, 1.0 / len))
}

/// Unit vector perpendicular to `unit`, chosen from the least-aligned coordinate axis.
pub(crate) fn any_perpendicular(unit: D3) -> D3 {
    let (ax, ay, az) = (unit[0].abs(), unit[1].abs(), unit[2].abs());
    let axis = if ax <= ay && ax <= az {
        [1.0, 0.0, 0.0]
    } else if ay <= az {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    normalized(cross(unit, axis)).unwrap_or([1.0, 0.0, 0.0])
}

/// Newell's area vector of a polygon: direction is the normal, magnitude is twice the area.
pub(crate) fn polygon_area_vector(points: &[D3], polygon: &[u32]) -> D3 {
    let mut total = [0.0; 3];
    for (i, &index) in polygon.iter().enumerate() {
        let a = points[index as usize];
        let b = points[polygon[(i + 1) % polygon.len()] as usize];
        total = add(total, cross(a, b));
    }
    total
}

/// Reads every point position in double precision.
pub(crate) fn load_points(geo: &GeoDetail) -> Result<Vec<D3>, GeoError> {
    (0..geo.len())
        .map(|i| geo.point(i).map(to_d3).ok_or(GeoError::IndexOutOfBounds))
        .collect()
}

/// A primitive's kind and its point indices, in storage order.
pub(crate) struct PrimView {
    pub kind: PrimKind,
    pub points: Vec<u32>,
}

pub(crate) fn load_prims(geo: &GeoDetail) -> Result<Vec<PrimView>, GeoError> {
    let mut prims = Vec::with_capacity(geo.prim_count());
    let mut cursor = 0usize;
    for run in geo.prim_runs() {
        let arity = run.arity as usize;
        for _ in 0..run.prim_count {
            let mut points = Vec::with_capacity(arity);
            for vertex in cursor..cursor + arity {
                points.push(geo.vertex_point(vertex).ok_or(GeoError::IndexOutOfBounds)?);
            }
            prims.push(PrimView {
                kind: run.kind,
                points,
            });
            cursor += arity;
        }
    }
    Ok(prims)
}

/// Point lists of the polygon primitives only.
pub(crate) fn load_polygons(geo: &GeoDetail) -> Result<Vec<Vec<u32>>, GeoError> {
    Ok(load_prims(geo)?
        .into_iter()
        .filter(|prim| prim.kind == PrimKind::Polygon)
        .map(|prim| prim.points)
        .collect())
}

/// Assembles `prims` over the existing points of `base`, run-length encoding equal kind and arity.
pub(crate) fn attach_prims(
    base: &GeoDetail,
    prims: &[(PrimKind, Vec<u32>)],
) -> Result<GeoDetail, GeoError> {
    let mut runs: Vec<PrimRun> = Vec::new();
    let mut vertex_points = Vec::with_capacity(prims.iter().map(|p| p.1.len()).sum());
    for (kind, points) in prims {
        let arity = u32::try_from(points.len()).map_err(|_| GeoError::LimitExceeded)?;
        match runs.last_mut() {
            Some(last) if last.kind == *kind && last.arity == arity => last.prim_count += 1,
            _ => runs.push(PrimRun {
                kind: *kind,
                arity,
                prim_count: 1,
            }),
        }
        vertex_points.extend_from_slice(points);
    }
    base.with_primitives(runs, vertex_points)
}

/// Builds a new detail from double-precision points and primitives, after a budget check.
pub(crate) fn build_geo(
    points: &[D3],
    prims: &[(PrimKind, Vec<u32>)],
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    let vertices = prims.iter().map(|p| p.1.len()).sum();
    ctx.check_output(points.len(), vertices)?;
    let positions = points
        .iter()
        .map(|&p| to_vec3(p))
        .collect::<Result<Vec<_>, _>>()?;
    attach_prims(&GeoDetail::points(positions), prims)
}

/// Whether two polygons use the same set of points. Back-to-back copies of a face pass every
/// edge-based manifold test, but they enclose nothing, so they are not a solid surface.
#[must_use]
pub fn has_repeated_faces(geo: &GeoDetail) -> bool {
    let Ok(polygons) = load_polygons(geo) else {
        return false;
    };
    let mut keys: Vec<Vec<u32>> = polygons
        .into_iter()
        .map(|mut polygon| {
            polygon.sort_unstable();
            polygon
        })
        .collect();
    keys.sort_unstable();
    keys.windows(2).any(|pair| pair[0] == pair[1])
}

/// Signed enclosed volume of the polygons: positive when closed surfaces face outward.
#[must_use]
pub fn signed_volume(geo: &GeoDetail) -> f64 {
    let (Ok(points), Ok(polygons)) = (load_points(geo), load_polygons(geo)) else {
        return 0.0;
    };
    let mut six_volume = 0.0;
    for polygon in &polygons {
        let origin = points[polygon[0] as usize];
        for k in 1..polygon.len() - 1 {
            let a = points[polygon[k] as usize];
            let b = points[polygon[k + 1] as usize];
            six_volume += dot(origin, cross(a, b));
        }
    }
    six_volume / 6.0
}

/// Total-order `f64` key for deterministic sorts and priority queues.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OrdF64(pub f64);

impl Eq for OrdF64 {}

impl PartialOrd for OrdF64 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrdF64 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}
