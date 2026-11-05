//! Packed instances: primitives that reference a shared, immutable prototype.
//!
//! An instance costs a fixed amount of memory however large its prototype is. Its world position is
//! `origin + rows * p`, where `origin` is its point. Transforms compose into `rows` and never touch
//! the prototype, and only an explicit unpack creates per-face geometry.

use crate::{
    AttrDomain, AttributeColumn, FiniteF32, GeoDetail, GeoError, PAGE_LEN, PagedColumn, PrimKind,
    PrimRun,
};
use pcc_math::Vec3;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// How many levels of instances of instances are allowed. Hashes and files recurse through these
/// levels, so the limit also bounds that recursion, however a graph or a file tries to nest them.
pub const MAX_PACKED_DEPTH: u32 = 32;

/// A shared prototype, with the facts about it that would otherwise need its whole geometry.
#[derive(Clone, Debug)]
pub struct PackedPrototype {
    geo: Arc<GeoDetail>,
    hash: blake3::Hash,
    bounds: Option<(Vec3, Vec3)>,
    depth: u32,
}

impl PackedPrototype {
    pub fn new(geo: Arc<GeoDetail>) -> Result<Self, GeoError> {
        let depth = geo.packed.as_ref().map_or(0, |table| {
            table.prototypes.iter().map(|p| p.depth).max().unwrap_or(0)
        }) + 1;
        if depth > MAX_PACKED_DEPTH {
            return Err(GeoError::LimitExceeded);
        }
        Ok(Self {
            hash: geo.hash(),
            bounds: geo.bounds(),
            geo,
            depth,
        })
    }

    #[must_use]
    pub fn geo(&self) -> &Arc<GeoDetail> {
        &self.geo
    }
}

/// One instance: which prototype it shows, and how.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PackedInstance {
    pub prototype: u32,
    /// The rows of the matrix that maps prototype space to world space, before translation.
    pub rows: [Vec3; 3],
}

/// Per-instance data, indexed by a packed primitive's ordinal among the packed primitives.
#[derive(Clone, Debug)]
pub(crate) struct PackedTable {
    prototypes: Arc<[PackedPrototype]>,
    prototype_of: PagedColumn<u32>,
    rows: [PagedColumn<Vec3>; 3],
}

impl PackedTable {
    fn new(prototypes: Vec<PackedPrototype>, instances: &[PackedInstance]) -> Self {
        let column = |row: usize| {
            PagedColumn::from_vec(instances.iter().map(|i| i.rows[row]).collect::<Vec<_>>())
        };
        Self {
            prototypes: Arc::from(prototypes),
            prototype_of: PagedColumn::from_vec(instances.iter().map(|i| i.prototype).collect()),
            rows: [column(0), column(1), column(2)],
        }
    }

    pub(crate) fn update_hash(&self, hasher: &mut blake3::Hasher) {
        hasher.update(b"packed");
        hasher.update(&(self.prototypes.len() as u64).to_le_bytes());
        for prototype in &*self.prototypes {
            hasher.update(prototype.hash.as_bytes());
        }
        hasher.update(self.prototype_of.hash().as_bytes());
        for row in &self.rows {
            hasher.update(row.hash().as_bytes());
        }
    }

    fn instance(&self, index: usize) -> Option<PackedInstance> {
        Some(PackedInstance {
            prototype: self.prototype_of.get(index)?,
            rows: [
                self.rows[0].get(index)?,
                self.rows[1].get(index)?,
                self.rows[2].get(index)?,
            ],
        })
    }

    fn instances(&self) -> impl Iterator<Item = PackedInstance> + '_ {
        (0..self.prototype_of.len()).filter_map(|index| self.instance(index))
    }

    /// Joins two tables, sharing prototypes with equal content.
    pub(crate) fn merged(first: Option<&Self>, second: Option<&Self>) -> Option<Self> {
        let mut prototypes: Vec<PackedPrototype> = Vec::new();
        let mut instances: Vec<PackedInstance> = Vec::new();
        for table in [first, second].into_iter().flatten() {
            let mut remap = Vec::with_capacity(table.prototypes.len());
            for prototype in &*table.prototypes {
                let found = prototypes.iter().position(|p| p.hash == prototype.hash);
                remap.push(found.unwrap_or_else(|| {
                    prototypes.push(prototype.clone());
                    prototypes.len() - 1
                }) as u32);
            }
            instances.extend(table.instances().map(|instance| PackedInstance {
                prototype: remap[instance.prototype as usize],
                rows: instance.rows,
            }));
        }
        (first.is_some() || second.is_some()).then(|| Self::new(prototypes, &instances))
    }
}

/// `a * b`, for matrices given as rows.
fn multiply(a: &[Vec3; 3], b: &[Vec3; 3]) -> Result<[Vec3; 3], GeoError> {
    let row = |r: Vec3| -> Result<Vec3, GeoError> {
        let column = |c: usize| {
            let at = |m: &Vec3| [m.x, m.y, m.z][c];
            f64::from(r.x) * f64::from(at(&b[0]))
                + f64::from(r.y) * f64::from(at(&b[1]))
                + f64::from(r.z) * f64::from(at(&b[2]))
        };
        Ok(Vec3::checked(
            column(0) as f32,
            column(1) as f32,
            column(2) as f32,
        )?)
    };
    Ok([row(a[0])?, row(a[1])?, row(a[2])?])
}

fn apply(rows: &[Vec3; 3], p: Vec3) -> [f64; 3] {
    let dot = |r: Vec3| {
        f64::from(r.x) * f64::from(p.x)
            + f64::from(r.y) * f64::from(p.y)
            + f64::from(r.z) * f64::from(p.z)
    };
    [dot(rows[0]), dot(rows[1]), dot(rows[2])]
}

/// Normals map by the inverse transpose, which is the cofactor matrix up to a scale that does not
/// matter once normalized. Unlike a true inverse, it still exists for a singular matrix.
fn map_normal(rows: &[Vec3; 3], n: Vec3) -> Result<Vec3, GeoError> {
    let [a, b, c] = *rows;
    let cofactor = [
        b.y * c.z - b.z * c.y,
        b.z * c.x - b.x * c.z,
        b.x * c.y - b.y * c.x,
        c.y * a.z - c.z * a.y,
        c.z * a.x - c.x * a.z,
        c.x * a.y - c.y * a.x,
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    ];
    let mapped = Vec3::checked(
        cofactor[0] * n.x + cofactor[1] * n.y + cofactor[2] * n.z,
        cofactor[3] * n.x + cofactor[4] * n.y + cofactor[5] * n.z,
        cofactor[6] * n.x + cofactor[7] * n.y + cofactor[8] * n.z,
    )?;
    Ok(mapped.normalized_or_up())
}

fn determinant(rows: &[Vec3; 3]) -> f64 {
    let [a, b, c] = rows.map(|r| [f64::from(r.x), f64::from(r.y), f64::from(r.z)]);
    a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
        + a[2] * (b[0] * c[1] - b[1] * c[0])
}

const IDENTITY: [Vec3; 3] = [
    Vec3 {
        x: 1.0,
        y: 0.0,
        z: 0.0,
    },
    Vec3 {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    },
    Vec3 {
        x: 0.0,
        y: 0.0,
        z: 1.0,
    },
];

impl GeoDetail {
    /// How many packed instances this geometry holds.
    #[must_use]
    pub fn packed_count(&self) -> usize {
        self.packed.as_ref().map_or(0, |t| t.prototype_of.len())
    }

    /// The distinct prototypes that instances refer to.
    #[must_use]
    pub fn packed_prototypes(&self) -> &[PackedPrototype] {
        self.packed.as_ref().map_or(&[], |t| &t.prototypes)
    }

    /// One instance, by its ordinal among the packed primitives.
    #[must_use]
    pub fn packed_instance(&self, index: usize) -> Option<PackedInstance> {
        self.packed.as_ref()?.instance(index)
    }

    /// Wraps all of this geometry into one instance at the origin.
    pub fn packed_whole(geo: Arc<Self>) -> Result<Self, GeoError> {
        let prototype = PackedPrototype::new(geo)?;
        Self::points(vec![Vec3::ZERO]).with_instances_on_points(prototype, &|_| 1.0)
    }

    /// Places one instance on every point, uniformly scaled by `scale(point)`. Points and their
    /// attributes are kept, and any other primitives on them are not.
    pub fn with_instances_on_points(
        &self,
        prototype: PackedPrototype,
        scale: &dyn Fn(usize) -> f32,
    ) -> Result<Self, GeoError> {
        let count = self.len();
        let mut instances = Vec::with_capacity(count);
        for point in 0..count {
            let s = FiniteF32::checked(scale(point))?.get();
            let row = |r: Vec3| Vec3 {
                x: r.x * s,
                y: r.y * s,
                z: r.z * s,
            };
            instances.push(PackedInstance {
                prototype: 0,
                rows: [row(IDENTITY[0]), row(IDENTITY[1]), row(IDENTITY[2])],
            });
        }
        let runs = if count == 0 {
            Vec::new()
        } else {
            vec![PrimRun {
                kind: PrimKind::Packed,
                arity: 1,
                prim_count: count,
            }]
        };
        let vertices = (0..u32::try_from(count).map_err(|_| GeoError::LimitExceeded)?).collect();
        let mut next = self.with_runs(runs, vertices)?;
        next.packed = Some(Arc::new(PackedTable::new(vec![prototype], &instances)));
        Ok(next)
    }

    /// Composes a linear map, given as rows, onto every instance. Prototypes are untouched.
    pub fn with_packed_linear(&self, map: &[Vec3; 3]) -> Result<Self, GeoError> {
        let Some(table) = &self.packed else {
            return Ok(self.clone());
        };
        let mut instances = Vec::with_capacity(self.packed_count());
        for instance in table.instances() {
            instances.push(PackedInstance {
                prototype: instance.prototype,
                rows: multiply(map, &instance.rows)?,
            });
        }
        let mut next = self.clone();
        next.packed = Some(Arc::new(PackedTable::new(
            table.prototypes.to_vec(),
            &instances,
        )));
        Ok(next)
    }

    /// Rebuilds geometry from stored parts, as when reading a file.
    pub fn with_packed_parts(
        &self,
        prototypes: Vec<Arc<Self>>,
        instances: &[PackedInstance],
    ) -> Result<Self, GeoError> {
        let expected: usize = self
            .prim_runs
            .iter()
            .filter(|run| run.kind == PrimKind::Packed)
            .map(|run| run.prim_count)
            .sum();
        if instances.len() != expected {
            return Err(GeoError::InvalidTopology);
        }
        if instances
            .iter()
            .any(|i| i.prototype as usize >= prototypes.len())
        {
            return Err(GeoError::IndexOutOfBounds);
        }
        let prototypes = prototypes
            .into_iter()
            .map(PackedPrototype::new)
            .collect::<Result<Vec<_>, _>>()?;
        let mut next = self.clone();
        next.packed = Some(Arc::new(PackedTable::new(prototypes, instances)));
        Ok(next)
    }

    /// Bounds that contain every instance, without visiting any prototype's geometry.
    pub(crate) fn packed_bounds(&self) -> Option<(Vec3, Vec3)> {
        let table = self.packed.as_ref()?;
        let (mut low, mut high) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        let mut cursor = 0usize;
        let mut ordinal = 0usize;
        for run in &*self.prim_runs {
            let arity = run.arity as usize;
            if run.kind != PrimKind::Packed {
                cursor += arity * run.prim_count;
                continue;
            }
            for _ in 0..run.prim_count {
                let instance = table.instance(ordinal)?;
                let origin = self.point(self.vertex_point(cursor)? as usize)?;
                ordinal += 1;
                cursor += arity;
                let prototype = table.prototypes.get(instance.prototype as usize)?;
                let Some((box_low, box_high)) = prototype.bounds else {
                    continue;
                };
                // The extent of a transformed box, taken per axis from the matrix entries.
                for (axis, (row, center)) in instance
                    .rows
                    .iter()
                    .zip([origin.x, origin.y, origin.z])
                    .enumerate()
                {
                    let (mut min, mut max) = (f64::from(center), f64::from(center));
                    let spans = [
                        (row.x, box_low.x, box_high.x),
                        (row.y, box_low.y, box_high.y),
                        (row.z, box_low.z, box_high.z),
                    ];
                    for (factor, from, to) in spans {
                        let (a, b) = (
                            f64::from(factor) * f64::from(from),
                            f64::from(factor) * f64::from(to),
                        );
                        min += a.min(b);
                        max += a.max(b);
                    }
                    low[axis] = low[axis].min(min);
                    high[axis] = high[axis].max(max);
                }
            }
        }
        if !low[0].is_finite() {
            return None;
        }
        Some((
            Vec3::checked(low[0] as f32, low[1] as f32, low[2] as f32).ok()?,
            Vec3::checked(high[0] as f32, high[1] as f32, high[2] as f32).ok()?,
        ))
    }

    /// Everything except the packed primitives, dropping points only they used.
    fn without_packed(&self) -> Result<Self, GeoError> {
        let mut used = vec![false; self.len()];
        let mut runs = Vec::new();
        let mut kept_prims = Vec::new();
        let mut kept_vertices = Vec::new();
        let mut vertices = Vec::new();
        let (mut cursor, mut prim) = (0usize, 0usize);
        for run in &*self.prim_runs {
            let arity = run.arity as usize;
            if run.kind == PrimKind::Packed {
                cursor += arity * run.prim_count;
                prim += run.prim_count;
                continue;
            }
            runs.push(*run);
            for _ in 0..run.prim_count {
                for vertex in cursor..cursor + arity {
                    let point = self
                        .vertex_point(vertex)
                        .ok_or(GeoError::IndexOutOfBounds)?;
                    used[point as usize] = true;
                    kept_vertices.push(vertex);
                    vertices.push(point);
                }
                kept_prims.push(prim);
                cursor += arity;
                prim += 1;
            }
        }
        let mut remap = vec![0u32; self.len()];
        let mut kept_points = Vec::new();
        for (point, _) in used.iter().enumerate().filter(|(_, used)| **used) {
            remap[point] = kept_points.len() as u32;
            kept_points.push(point);
        }
        let positions = kept_points
            .iter()
            .map(|&p| self.point(p).ok_or(GeoError::IndexOutOfBounds))
            .collect::<Result<Vec<_>, _>>()?;
        let vertices = vertices.into_iter().map(|p| remap[p as usize]).collect();
        let mut out = Self::points(positions).with_runs(runs, vertices)?;
        let domains = [
            (AttrDomain::Point, &kept_points),
            (AttrDomain::Vertex, &kept_vertices),
            (AttrDomain::Prim, &kept_prims),
        ];
        for (domain, kept) in domains {
            for name in self.domain_attribute_names(domain) {
                if let Some(column) = self.domain_attribute(domain, name) {
                    out = out.with_domain_attribute(domain, name, column.selected(kept)?)?;
                }
            }
        }
        for name in self.domain_attribute_names(AttrDomain::Detail) {
            if let Some(column) = self.domain_attribute(AttrDomain::Detail, name) {
                out = out.with_domain_attribute(AttrDomain::Detail, name, column.clone())?;
            }
        }
        Ok(out)
    }

    /// Expands one level of instances into real geometry, keeping attributes, with `N` following
    /// each instance's transform. Fails before allocating if the result would exceed the budget.
    pub fn unpacked_cancellable(
        &self,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        let Some(table) = &self.packed else {
            return Ok(self.clone());
        };
        let mut placed = Vec::with_capacity(self.packed_count());
        let mut mirrored: BTreeMap<u32, Arc<Self>> = BTreeMap::new();
        let (mut total_points, mut total_vertices, mut total_prims) = (0usize, 0usize, 0usize);
        let mut cursor = 0usize;
        let mut ordinal = 0usize;
        for run in &*self.prim_runs {
            let arity = run.arity as usize;
            if run.kind != PrimKind::Packed {
                cursor += arity * run.prim_count;
                continue;
            }
            for _ in 0..run.prim_count {
                let instance = table.instance(ordinal).ok_or(GeoError::IndexOutOfBounds)?;
                let origin = self
                    .vertex_point(cursor)
                    .and_then(|p| self.point(p as usize))
                    .ok_or(GeoError::IndexOutOfBounds)?;
                let prototype = table
                    .prototypes
                    .get(instance.prototype as usize)
                    .ok_or(GeoError::IndexOutOfBounds)?;
                // A mirroring transform turns surfaces inside out unless winding reverses with it.
                let geo = if determinant(&instance.rows) < 0.0 {
                    let cached = mirrored.entry(instance.prototype).or_insert_with(|| {
                        prototype
                            .geo
                            .with_reversed_winding()
                            .map(Arc::new)
                            .unwrap_or_else(|_| Arc::clone(&prototype.geo))
                    });
                    Arc::clone(cached)
                } else {
                    Arc::clone(&prototype.geo)
                };
                let add = |total: usize, more: usize| {
                    total.checked_add(more).ok_or(GeoError::LimitExceeded)
                };
                total_points = add(total_points, geo.len())?;
                total_vertices = add(total_vertices, geo.vertex_count())?;
                total_prims = add(total_prims, geo.prim_count())?;
                placed.push((geo, instance.rows, origin));
                ordinal += 1;
                cursor += arity;
            }
        }
        let bytes = total_points
            .checked_mul(std::mem::size_of::<Vec3>())
            .and_then(|b| b.checked_add(total_vertices.checked_mul(std::mem::size_of::<u32>())?))
            .ok_or(GeoError::LimitExceeded)?;
        if bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }
        let mut positions = Vec::with_capacity(total_points);
        let mut vertices = Vec::with_capacity(total_vertices);
        let mut runs: Vec<PrimRun> = Vec::new();
        for (index, (geo, rows, origin)) in placed.iter().enumerate() {
            if index.is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            let offset = u32::try_from(positions.len()).map_err(|_| GeoError::LimitExceeded)?;
            for point in 0..geo.len() {
                let p = geo.point(point).ok_or(GeoError::IndexOutOfBounds)?;
                let [x, y, z] = apply(rows, p);
                positions.push(Vec3::checked(
                    (x + f64::from(origin.x)) as f32,
                    (y + f64::from(origin.y)) as f32,
                    (z + f64::from(origin.z)) as f32,
                )?);
            }
            for vertex in 0..geo.vertex_count() {
                let point = geo.vertex_point(vertex).ok_or(GeoError::IndexOutOfBounds)?;
                vertices.push(point.checked_add(offset).ok_or(GeoError::LimitExceeded)?);
            }
            for run in geo.prim_runs() {
                match runs.last_mut() {
                    Some(last) if last.kind == run.kind && last.arity == run.arity => {
                        last.prim_count += run.prim_count;
                    }
                    _ => runs.push(*run),
                }
            }
        }
        // One level only: instances inside a prototype stay instances.
        let nested =
            PackedTable::merged_all(placed.iter().map(|(geo, _, _)| geo.packed.as_deref()));
        let mut expanded = Self::points(positions).with_runs(runs, vertices)?;
        expanded.packed = nested.map(Arc::new);
        for domain in [AttrDomain::Point, AttrDomain::Vertex, AttrDomain::Prim] {
            let names: BTreeSet<&str> = placed
                .iter()
                .flat_map(|(geo, _, _)| geo.domain_attribute_names(domain))
                .collect();
            for name in names {
                let parts: Vec<(Option<&AttributeColumn>, usize)> = placed
                    .iter()
                    .map(|(geo, _, _)| (geo.domain_attribute(domain, name), geo.domain_len(domain)))
                    .collect();
                let mut column = AttributeColumn::concatenated_all(&parts)?;
                if name == "N"
                    && let AttributeColumn::Vec3(values) = &column
                {
                    let mut mapped = Vec::with_capacity(values.len());
                    let mut at = 0usize;
                    for (geo, rows, _) in &placed {
                        for _ in 0..geo.domain_len(domain) {
                            let n = values.get(at).ok_or(GeoError::IndexOutOfBounds)?;
                            mapped.push(map_normal(rows, n)?);
                            at += 1;
                        }
                    }
                    column = AttributeColumn::vec3_values(mapped);
                }
                expanded = expanded.with_domain_attribute(domain, name, column)?;
            }
        }
        for (geo, _, _) in &placed {
            for name in geo.domain_attribute_names(AttrDomain::Detail) {
                if expanded
                    .domain_attribute(AttrDomain::Detail, name)
                    .is_none()
                    && let Some(column) = geo.domain_attribute(AttrDomain::Detail, name)
                {
                    expanded =
                        expanded.with_domain_attribute(AttrDomain::Detail, name, column.clone())?;
                }
            }
        }
        // Whatever else was here stays, in front of the expanded instances.
        let plain = self.without_packed()?;
        if plain.is_empty() {
            Ok(expanded)
        } else {
            plain.merged_cancellable(&expanded, memory_limit, cancelled)
        }
    }
}

impl PackedTable {
    /// Joins any number of tables, for instances nested one level down.
    fn merged_all<'a>(tables: impl Iterator<Item = Option<&'a Self>>) -> Option<Self> {
        let mut merged: Option<Self> = None;
        for table in tables.flatten() {
            merged = Self::merged(merged.as_ref(), Some(table));
        }
        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    /// A closed unit tetrahedron with an attribute, a normal, and a detail value.
    fn prototype() -> Result<Arc<GeoDetail>, GeoError> {
        let geo = GeoDetail::points(vec![
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(0.0, 1.0, 0.0),
            v(0.0, 0.0, 1.0),
        ])
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 4,
            }],
            vec![0, 2, 1, 0, 1, 3, 1, 2, 3, 0, 3, 2],
        )?
        .with_attribute(
            "mass",
            AttributeColumn::f32_checked(vec![1.0, 2.0, 3.0, 4.0])?,
        )?
        .with_attribute("N", AttributeColumn::vec3_values(vec![v(0.0, 0.0, 1.0); 4]))?
        .with_domain_attribute(
            AttrDomain::Prim,
            "tag",
            AttributeColumn::i32_values(vec![5; 4]),
        )?
        .with_domain_attribute(
            AttrDomain::Detail,
            "note",
            AttributeColumn::i32_values(vec![9]),
        )?;
        Ok(Arc::new(geo))
    }

    fn cloud(points: &[Vec3]) -> GeoDetail {
        GeoDetail::points(points.to_vec())
    }

    #[test]
    fn instances_cost_a_fixed_amount_and_share_one_prototype() -> Result<(), GeoError> {
        let proto = prototype()?;
        let placed = cloud(&[v(0.0, 0.0, 0.0), v(5.0, 0.0, 0.0), v(0.0, 5.0, 0.0)])
            .with_attribute("id", AttributeColumn::i32_values(vec![7, 8, 9]))?
            .with_instances_on_points(PackedPrototype::new(Arc::clone(&proto))?, &|i| {
                i as f32 + 1.0
            })?;
        assert_eq!(placed.packed_count(), 3);
        assert_eq!(placed.prim_count(), 3);
        assert_eq!(placed.len(), 3, "one point per instance, not four");
        assert_eq!(placed.packed_prototypes().len(), 1);
        assert!(Arc::ptr_eq(placed.packed_prototypes()[0].geo(), &proto));
        // Point attributes stay on the instancing points.
        assert!(placed.attribute("id").is_some());
        let second = placed
            .packed_instance(1)
            .ok_or(GeoError::IndexOutOfBounds)?;
        assert_eq!(second.rows[0], v(2.0, 0.0, 0.0));
        assert_eq!(second.rows[2], v(0.0, 0.0, 2.0));
        Ok(())
    }

    #[test]
    fn unpacking_places_each_copy_and_keeps_its_attributes() -> Result<(), GeoError> {
        let cancelled = AtomicBool::new(false);
        let placed = cloud(&[v(10.0, 0.0, 0.0), v(0.0, 10.0, 0.0)])
            .with_instances_on_points(PackedPrototype::new(prototype()?)?, &|i| i as f32 + 1.0)?;
        let flat = placed.unpacked_cancellable(1 << 20, &cancelled)?;
        assert_eq!(flat.packed_count(), 0);
        assert_eq!((flat.len(), flat.prim_count()), (8, 8));
        // Second copy: scale two, at (0, 10, 0).
        assert_eq!(flat.point(5), Some(v(2.0, 10.0, 0.0)));
        // One-sixth, then two cubed times that: analytic volumes of the two tetrahedra.
        assert!((flat.enclosed_volume() - (1.0 + 8.0) / 6.0).abs() < 1e-5);
        assert!(flat.topology_index().is_closed_manifold());
        let mass = flat.attribute("mass").and_then(AttributeColumn::as_f32);
        assert_eq!(mass.and_then(|c| c.get(5)).map(|f| f.get()), Some(2.0));
        let tag = flat
            .domain_attribute(AttrDomain::Prim, "tag")
            .and_then(AttributeColumn::as_i32);
        assert_eq!(tag.and_then(|c| c.get(7)), Some(5));
        let note = flat
            .domain_attribute(AttrDomain::Detail, "note")
            .and_then(AttributeColumn::as_i32);
        assert_eq!(note.and_then(|c| c.get(0)), Some(9));
        Ok(())
    }

    #[test]
    fn transforms_compose_exactly_and_normals_use_the_inverse_transpose() -> Result<(), GeoError> {
        let cancelled = AtomicBool::new(false);
        let placed = cloud(&[v(1.0, 2.0, 3.0)])
            .with_instances_on_points(PackedPrototype::new(prototype()?)?, &|_| 1.0)?;
        // Non-uniform scale: x doubled. Then a quarter turn about Z.
        let scale = [v(2.0, 0.0, 0.0), v(0.0, 1.0, 0.0), v(0.0, 0.0, 1.0)];
        let turn = [v(0.0, -1.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0)];
        let moved = placed
            .with_packed_linear(&scale)?
            .with_packed_linear(&turn)?;
        let rows = moved
            .packed_instance(0)
            .ok_or(GeoError::IndexOutOfBounds)?
            .rows;
        // The turn applies second, so it multiplies from the left.
        assert_eq!(
            rows,
            [v(0.0, -1.0, 0.0), v(2.0, 0.0, 0.0), v(0.0, 0.0, 1.0)]
        );
        let flat = moved.unpacked_cancellable(1 << 20, &cancelled)?;
        // Prototype point (1, 0, 0) becomes (0, 2, 0), offset by the point at (1, 2, 3).
        assert_eq!(flat.point(1), Some(v(1.0, 4.0, 3.0)));
        // Prototype normals stayed untouched, and the unpacked ones were mapped.
        let n = [v(0.70710677, 0.70710677, 0.0)];
        let slanted =
            cloud(&[Vec3::ZERO]).with_attribute("N", AttributeColumn::vec3_values(n.to_vec()))?;
        let packed = GeoDetail::packed_whole(Arc::new(slanted))?.with_packed_linear(&scale)?;
        let mapped = packed.unpacked_cancellable(1 << 20, &cancelled)?;
        let normal = mapped
            .attribute("N")
            .and_then(AttributeColumn::as_vec3)
            .and_then(|c| c.get(0))
            .ok_or(GeoError::AttributeNotFound)?;
        // A plane with normal (1, 1, 0) stretched along X has normal proportional to (1, 2, 0).
        assert!((normal.y / normal.x - 2.0).abs() < 1e-5, "{normal:?}");
        assert!((normal.length() - 1.0).abs() < 1e-5);
        Ok(())
    }

    #[test]
    fn bounds_contain_instances_without_expanding_them() -> Result<(), GeoError> {
        let cancelled = AtomicBool::new(false);
        let placed = cloud(&[v(10.0, 0.0, 0.0), v(-10.0, 4.0, 0.0)])
            .with_instances_on_points(PackedPrototype::new(prototype()?)?, &|_| 3.0)?;
        let bounds = placed.bounds();
        // Axis-aligned, so exact: the same as what unpacking really produces.
        let flat = placed.unpacked_cancellable(1 << 20, &cancelled)?;
        assert_eq!(bounds, flat.bounds());
        assert_eq!(bounds.map(|(_, high)| high.x), Some(13.0));
        // Rotated, bounds are conservative: they contain every real point.
        let angle = 0.5f32;
        let (sin, cos) = angle.sin_cos();
        let turn = [v(cos, -sin, 0.0), v(sin, cos, 0.0), v(0.0, 0.0, 1.0)];
        let turned = placed.with_packed_linear(&turn)?;
        let (low, high) = turned.bounds().ok_or(GeoError::IndexOutOfBounds)?;
        let flat = turned.unpacked_cancellable(1 << 20, &cancelled)?;
        for i in 0..flat.len() {
            let p = flat.point(i).ok_or(GeoError::IndexOutOfBounds)?;
            assert!(p.x >= low.x - 1e-4 && p.x <= high.x + 1e-4, "{p:?}");
            assert!(p.y >= low.y - 1e-4 && p.y <= high.y + 1e-4, "{p:?}");
            assert!(p.z >= low.z - 1e-4 && p.z <= high.z + 1e-4, "{p:?}");
        }
        // An empty prototype takes no space, and the points alone set the bounds.
        let empty = cloud(&[v(1.0, 1.0, 1.0)])
            .with_instances_on_points(PackedPrototype::new(Arc::new(cloud(&[])))?, &|_| 1.0)?;
        assert_eq!(empty.bounds(), Some((v(1.0, 1.0, 1.0), v(1.0, 1.0, 1.0))));
        Ok(())
    }

    #[test]
    fn hashes_follow_instances_and_prototypes_and_merging_shares_equal_content()
    -> Result<(), GeoError> {
        let proto = prototype()?;
        let make = |x: f32, s: f32, proto: &Arc<GeoDetail>| -> Result<GeoDetail, GeoError> {
            cloud(&[v(x, 0.0, 0.0)])
                .with_instances_on_points(PackedPrototype::new(Arc::clone(proto))?, &|_| s)
        };
        let base = make(1.0, 1.0, &proto)?;
        assert_eq!(base.hash(), make(1.0, 1.0, &proto)?.hash());
        assert_ne!(base.hash(), make(2.0, 1.0, &proto)?.hash());
        assert_ne!(base.hash(), make(1.0, 2.0, &proto)?.hash());
        let other = Arc::new(cloud(&[v(0.5, 0.5, 0.5)]));
        assert_ne!(base.hash(), make(1.0, 1.0, &other)?.hash());
        // A copy of the prototype is the same content, and is the same instance.
        assert_eq!(
            base.hash(),
            make(1.0, 1.0, &Arc::new((*proto).clone()))?.hash()
        );

        let merged = base.merged(&make(5.0, 2.0, &Arc::new((*proto).clone()))?, usize::MAX)?;
        assert_eq!(merged.packed_count(), 2);
        assert_eq!(
            merged.packed_prototypes().len(),
            1,
            "equal content is one prototype"
        );
        let two = merged.merged(&make(9.0, 1.0, &other)?, usize::MAX)?;
        assert_eq!(two.packed_prototypes().len(), 2);
        assert_eq!(two.packed_instance(2).map(|i| i.prototype), Some(1));
        // Each instance keeps its own point through the merge.
        let cancelled = AtomicBool::new(false);
        let flat = two.unpacked_cancellable(1 << 20, &cancelled)?;
        assert_eq!(flat.len(), 4 + 4 + 1);
        assert_eq!(flat.point(8), Some(v(9.5, 0.5, 0.5)));
        Ok(())
    }

    #[test]
    fn mixed_geometry_keeps_its_plain_primitives_and_drops_only_the_instancing_points()
    -> Result<(), GeoError> {
        let cancelled = AtomicBool::new(false);
        let plain = (*prototype()?).clone();
        let placed = cloud(&[v(50.0, 0.0, 0.0)])
            .with_instances_on_points(PackedPrototype::new(prototype()?)?, &|_| 1.0)?;
        let mixed = plain.merged(&placed, usize::MAX)?;
        assert_eq!(
            (mixed.len(), mixed.prim_count(), mixed.packed_count()),
            (5, 5, 1)
        );
        let flat = mixed.unpacked_cancellable(1 << 20, &cancelled)?;
        assert_eq!(
            (flat.len(), flat.prim_count(), flat.packed_count()),
            (8, 8, 0)
        );
        assert_eq!(flat.point(4), Some(v(50.0, 0.0, 0.0)));
        // Plain attributes survive alongside the expanded ones.
        let tag = flat
            .domain_attribute(AttrDomain::Prim, "tag")
            .and_then(AttributeColumn::as_i32);
        assert_eq!(
            (tag.and_then(|c| c.get(0)), tag.and_then(|c| c.get(7))),
            (Some(5), Some(5))
        );
        Ok(())
    }

    #[test]
    fn unpacking_goes_one_level_and_nesting_is_bounded() -> Result<(), GeoError> {
        let cancelled = AtomicBool::new(false);
        let inner = cloud(&[v(1.0, 0.0, 0.0)])
            .with_instances_on_points(PackedPrototype::new(prototype()?)?, &|_| 1.0)?;
        let outer = cloud(&[v(0.0, 2.0, 0.0)])
            .with_instances_on_points(PackedPrototype::new(Arc::new(inner))?, &|_| 1.0)?;
        let once = outer.unpacked_cancellable(1 << 20, &cancelled)?;
        assert_eq!(
            once.packed_count(),
            1,
            "the inner instance stays an instance"
        );
        assert_eq!(once.point(0), Some(v(1.0, 2.0, 0.0)));
        let twice = once.unpacked_cancellable(1 << 20, &cancelled)?;
        assert_eq!(twice.packed_count(), 0);
        assert_eq!(twice.point(1), Some(v(2.0, 2.0, 0.0)));

        // Nesting without end must stop with an error rather than overflow the stack later.
        let mut nest = GeoDetail::packed_whole(prototype()?)?;
        let mut depth = 1;
        loop {
            match GeoDetail::packed_whole(Arc::new(nest.clone())) {
                Ok(deeper) => {
                    nest = deeper;
                    depth += 1;
                }
                Err(error) => {
                    assert_eq!(error, GeoError::LimitExceeded);
                    break;
                }
            }
            assert!(depth <= MAX_PACKED_DEPTH, "nesting was never refused");
        }
        assert_eq!(depth, MAX_PACKED_DEPTH);
        // The deepest allowed still hashes and bounds without trouble.
        assert!(nest.bounds().is_some());
        let _ = nest.hash();
        Ok(())
    }

    #[test]
    fn budgets_stop_an_unpack_before_it_allocates() -> Result<(), GeoError> {
        let cancelled = AtomicBool::new(false);
        let points = vec![Vec3::ZERO; 1000];
        let placed = cloud(&points)
            .with_instances_on_points(PackedPrototype::new(prototype()?)?, &|_| 1.0)?;
        // 1000 copies of 4 points and 12 vertices: 96,000 bytes.
        assert_eq!(
            placed.unpacked_cancellable(95_999, &cancelled).err(),
            Some(GeoError::LimitExceeded)
        );
        assert!(placed.unpacked_cancellable(96_000, &cancelled).is_ok());
        cancelled.store(true, Ordering::Release);
        assert_eq!(
            placed.unpacked_cancellable(1 << 20, &cancelled).err(),
            Some(GeoError::Cancelled)
        );
        Ok(())
    }

    #[test]
    fn packed_runs_only_come_from_the_packed_api() -> Result<(), GeoError> {
        // Hand-built packed primitives have no table to describe them.
        let forged = cloud(&[Vec3::ZERO]).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Packed,
                arity: 1,
                prim_count: 1,
            }],
            vec![0],
        );
        assert_eq!(forged.err(), Some(GeoError::InvalidTopology));
        // Replacing primitives on an instanced geometry removes its table with them.
        let placed = cloud(&[Vec3::ZERO])
            .with_instances_on_points(PackedPrototype::new(prototype()?)?, &|_| 1.0)?;
        let cleared = placed.with_primitives(Vec::new(), Vec::new())?;
        assert_eq!(cleared.packed_count(), 0);
        assert_eq!(cleared.packed_instance(0), None);
        // Parts read from a file must agree with the runs they describe.
        let shell = cloud(&[Vec3::ZERO]).with_runs(
            vec![PrimRun {
                kind: PrimKind::Packed,
                arity: 1,
                prim_count: 1,
            }],
            vec![0],
        )?;
        let good = PackedInstance {
            prototype: 0,
            rows: IDENTITY,
        };
        assert_eq!(
            shell.with_packed_parts(vec![prototype()?], &[]).err(),
            Some(GeoError::InvalidTopology)
        );
        let bad_index = PackedInstance {
            prototype: 5,
            rows: IDENTITY,
        };
        assert_eq!(
            shell
                .with_packed_parts(vec![prototype()?], &[bad_index])
                .err(),
            Some(GeoError::IndexOutOfBounds)
        );
        assert!(shell.with_packed_parts(vec![prototype()?], &[good]).is_ok());
        Ok(())
    }

    /// No entry is zero or one, so that every term of a formula shows in its result. Its
    /// determinant is exactly 0.25.
    fn general() -> [Vec3; 3] {
        [v(2.0, -1.5, 3.0), v(0.5, 4.0, -2.0), v(3.0, -3.0, 5.0)]
    }

    fn cross(a: Vec3, b: Vec3) -> Vec3 {
        v(
            a.y * b.z - a.z * b.y,
            a.z * b.x - a.x * b.z,
            a.x * b.y - a.y * b.x,
        )
    }

    fn assert_close(actual: Vec3, expected: Vec3) {
        let far = (actual.x - expected.x).abs()
            + (actual.y - expected.y).abs()
            + (actual.z - expected.z).abs();
        assert!(far < 1e-4, "{actual:?} is not {expected:?}");
    }

    #[test]
    fn matrix_helpers_match_worked_examples() -> Result<(), GeoError> {
        let m = general();
        assert_eq!(determinant(&m), 0.25);
        assert_eq!(determinant(&[m[1], m[0], m[2]]), -0.25);
        let a = [v(1.0, 2.0, 3.0), v(4.0, 5.0, 6.0), v(7.0, 8.0, 10.0)];
        let b = [v(2.0, 0.0, 1.0), v(1.0, 3.0, 0.0), v(0.0, 1.0, 4.0)];
        assert_eq!(
            multiply(&a, &b)?,
            [v(4.0, 9.0, 13.0), v(13.0, 21.0, 28.0), v(22.0, 34.0, 47.0)]
        );
        Ok(())
    }

    /// The cofactor matrix satisfies `cof(M)(a x b) = Ma x Mb`, which needs no inverse to check: a
    /// mapped normal must be the normal of the mapped tangents, whatever the transform.
    #[test]
    fn normals_stay_perpendicular_under_a_general_transform() -> Result<(), GeoError> {
        let m = general();
        let mirrored = [m[1], m[0], m[2]];
        let pairs = [
            (v(1.0, 2.0, -1.0), v(3.0, -1.0, 2.0)),
            (v(-2.0, 0.5, 3.0), v(1.0, 4.0, 1.5)),
        ];
        let image = |rows: &[Vec3; 3], p: Vec3| {
            let [x, y, z] = apply(rows, p);
            v(x as f32, y as f32, z as f32)
        };
        for rows in [m, mirrored] {
            for (first, second) in pairs {
                let normal = cross(first, second);
                let expected = cross(image(&rows, first), image(&rows, second));
                assert_close(map_normal(&rows, normal)?, expected.normalized_or_up());
            }
        }
        Ok(())
    }

    #[test]
    fn each_unpacked_normal_follows_its_own_instance() -> Result<(), GeoError> {
        let cancelled = AtomicBool::new(false);
        let normals = [v(1.0, 2.0, 3.0), v(-3.0, 1.0, 2.0)];
        let proto = cloud(&[v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)])
            .with_attribute("N", AttributeColumn::vec3_values(normals.to_vec()))?;
        let whole = GeoDetail::packed_whole(Arc::new(proto))?;
        let m = general();
        let turn = [v(0.0, -1.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0)];
        let flat = whole
            .with_packed_linear(&m)?
            .merged(&whole.with_packed_linear(&turn)?, usize::MAX)?
            .unpacked_cancellable(1 << 20, &cancelled)?;
        let column = flat
            .attribute("N")
            .and_then(AttributeColumn::as_vec3)
            .ok_or(GeoError::AttributeNotFound)?;
        let expected = [(0, &m, 0), (1, &m, 1), (2, &turn, 0), (3, &turn, 1)];
        for (index, rows, normal) in expected {
            let found = column.get(index).ok_or(GeoError::IndexOutOfBounds)?;
            assert_close(found, map_normal(rows, normals[normal])?);
        }
        Ok(())
    }

    #[test]
    fn only_a_truly_mirroring_transform_reverses_winding() -> Result<(), GeoError> {
        let cancelled = AtomicBool::new(false);
        let proto = prototype()?;
        let order = |geo: &GeoDetail| -> Vec<Option<u32>> {
            (0..geo.vertex_count())
                .map(|vertex| geo.vertex_point(vertex))
                .collect()
        };
        let unpack_with = |rows: &[Vec3; 3]| {
            GeoDetail::packed_whole(Arc::clone(&proto))?
                .with_packed_linear(rows)?
                .unpacked_cancellable(1 << 20, &cancelled)
        };
        let kept = order(&proto);
        // A flattening transform has a determinant of exactly zero, and mirrors nothing.
        let flatten = [v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0), v(0.0, 0.0, 0.0)];
        assert_eq!(order(&unpack_with(&flatten)?), kept);
        let turn = [v(0.0, -1.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0)];
        assert_eq!(order(&unpack_with(&turn)?), kept);
        let mirror = [v(-1.0, 0.0, 0.0), v(0.0, 1.0, 0.0), v(0.0, 0.0, 1.0)];
        let reflected = unpack_with(&mirror)?;
        assert_ne!(order(&reflected), kept);
        assert!(reflected.enclosed_volume() > 0.0);
        Ok(())
    }

    #[test]
    fn bounds_read_each_instance_past_plain_geometry() -> Result<(), GeoError> {
        // A box that neither contains the origin nor has a corner at one.
        let proto = Arc::new(cloud(&[v(0.5, 0.5, 0.5), v(2.0, 2.0, 2.0)]));
        let plain = (*prototype()?).clone();
        let scale = |i: usize| if i == 0 { 1.0 } else { 3.0 };
        let placed = cloud(&[v(50.0, 0.0, 0.0), v(-40.0, 10.0, 0.0)])
            .with_instances_on_points(PackedPrototype::new(proto)?, &scale)?;
        let mixed = plain.merged(&placed, usize::MAX)?;
        // First instance: x reaches 50 + 2. Second, at scale 3: y reaches 10 + 6, and z 0 + 6.
        // Points set the rest: x from -40 at the second instancing point, y and z from 0.
        assert_eq!(
            mixed.bounds(),
            Some((v(-40.0, 0.0, 0.0), v(52.0, 16.0, 6.0)))
        );
        Ok(())
    }

    #[test]
    fn plain_geometry_after_instances_keeps_its_own_attributes() -> Result<(), GeoError> {
        let cancelled = AtomicBool::new(false);
        let plain = GeoDetail::points(vec![
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(0.0, 1.0, 0.0),
            v(0.0, 0.0, 1.0),
        ])
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 2,
            }],
            vec![0, 2, 1, 1, 2, 3],
        )?
        .with_domain_attribute(
            AttrDomain::Prim,
            "tag",
            AttributeColumn::i32_values(vec![1, 2]),
        )?;
        let placed = cloud(&[v(50.0, 0.0, 0.0), v(60.0, 0.0, 0.0)]).with_instances_on_points(
            PackedPrototype::new(Arc::new(cloud(&[v(1.0, 1.0, 1.0)])))?,
            &|_| 1.0,
        )?;
        let flat = placed
            .merged(&plain, usize::MAX)?
            .unpacked_cancellable(1 << 20, &cancelled)?;
        // The plain part goes in front, with only its own points, its winding, and its tags.
        assert_eq!(flat.point(3), Some(v(0.0, 0.0, 1.0)));
        assert_eq!((flat.len(), flat.prim_count()), (6, 2));
        let order: Vec<_> = (0..flat.vertex_count())
            .map(|vertex| flat.vertex_point(vertex))
            .collect();
        let expected: Vec<_> = [0, 2, 1, 1, 2, 3].map(Some).to_vec();
        assert_eq!(order, expected);
        let tag = flat
            .domain_attribute(AttrDomain::Prim, "tag")
            .and_then(AttributeColumn::as_i32)
            .ok_or(GeoError::AttributeNotFound)?;
        assert_eq!((tag.get(0), tag.get(1)), (Some(1), Some(2)));
        Ok(())
    }

    #[test]
    fn unpacking_joins_equal_runs_and_keeps_unlike_ones_apart() -> Result<(), GeoError> {
        let cancelled = AtomicBool::new(false);
        let triangles = |count| PrimRun {
            kind: PrimKind::Polygon,
            arity: 3,
            prim_count: count,
        };
        let polyline = PrimRun {
            kind: PrimKind::Polyline,
            arity: 3,
            prim_count: 1,
        };
        let shape = || cloud(&[v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)]);
        let twice = |proto: GeoDetail| -> Result<GeoDetail, GeoError> {
            cloud(&[v(5.0, 0.0, 0.0), v(9.0, 0.0, 0.0)])
                .with_instances_on_points(PackedPrototype::new(Arc::new(proto))?, &|_| 1.0)?
                .unpacked_cancellable(1 << 20, &cancelled)
        };
        let uniform = twice(shape().with_primitives(vec![triangles(1)], vec![0, 1, 2])?)?;
        assert_eq!(uniform.prim_runs(), &[triangles(2)]);
        // Same arity, different kinds: joining them would turn a polyline into a polygon.
        let unlike =
            twice(shape().with_primitives(vec![triangles(1), polyline], vec![0, 1, 2, 0, 1, 2])?)?;
        assert_eq!(
            unlike.prim_runs(),
            &[triangles(1), polyline, triangles(1), polyline]
        );
        Ok(())
    }
}
