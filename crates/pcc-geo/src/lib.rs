//! Paged copy-on-write geometry detail and validated typed attribute columns.
//! Page starts are logical 16,384-element boundaries (`PAGE_LEN`).

mod packed;
mod tets;
mod volume;

use packed::PackedTable;
pub use packed::{MAX_PACKED_DEPTH, PackedInstance, PackedPrototype};
use volume::VolumeEntry;
pub use volume::{BRICK_DIM, BRICK_LEN, Brick8x8x8, VdbGrid, local_index, split_coord};

use pcc_math::{MathError, Vec3};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub const PAGE_LEN: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FiniteF32(f32);

impl FiniteF32 {
    pub const ZERO: Self = Self(0.0);

    pub fn checked(value: f32) -> Result<Self, GeoError> {
        if value.is_finite() {
            Ok(Self(if value == 0.0 { 0.0 } else { value }))
        } else {
            Err(GeoError::NonFinite)
        }
    }

    #[must_use]
    pub const fn get(self) -> f32 {
        self.0
    }
}

#[derive(Clone, Debug)]
pub struct Page<T> {
    values: Arc<[T]>,
    hash: blake3::Hash,
}

#[derive(Clone, Debug)]
pub struct PagedColumn<T> {
    pages: Arc<[Page<T>]>,
    len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttrDomain {
    Point,
    Vertex,
    Prim,
    Detail,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttrType {
    F32,
    I32,
    U32,
    Bool,
    Vec3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimKind {
    Polygon,
    Polyline,
    /// An instance of a shared prototype. Arity one: its vertex is the instance's point.
    Packed,
    /// A solid element with exactly four corners. See the `tets` module for its orientation.
    Tetrahedron,
    /// A sparse grid of voxels. Arity one: its vertex is the point that anchors it.
    Volume,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrimRun {
    pub kind: PrimKind,
    pub arity: u32,
    pub prim_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupDomain {
    Point,
    Prim,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromoteMode {
    Average,
    Min,
    Max,
    Sum,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UvProjectMode {
    PlanarY,
    PlanarZ,
    PlanarX,
    Cylindrical,
    Spherical,
    Box,
}

#[derive(Clone, Debug)]
pub struct ElementGroup {
    mask: PagedColumn<bool>,
    selected_count: usize,
}

impl ElementGroup {
    #[must_use]
    pub const fn len(&self) -> usize {
        self.mask.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.selected_count == 0
    }

    #[must_use]
    pub const fn selected_count(&self) -> usize {
        self.selected_count
    }

    #[must_use]
    pub fn contains(&self, index: usize) -> bool {
        self.mask.get(index).unwrap_or(false)
    }

    #[must_use]
    pub fn hash(&self) -> blake3::Hash {
        self.mask.hash()
    }
}

#[derive(Clone, Debug)]
pub enum AttributeColumn {
    F32(PagedColumn<FiniteF32>),
    I32(PagedColumn<i32>),
    U32(PagedColumn<u32>),
    Bool(PagedColumn<bool>),
    Vec3(PagedColumn<Vec3>),
}

#[derive(Clone, Debug)]
pub struct GeoDetail {
    positions: PagedColumn<Vec3>,
    vertex_points: PagedColumn<u32>,
    prim_runs: Arc<[PrimRun]>,
    prim_count: usize,
    point_attributes: Arc<BTreeMap<String, AttributeColumn>>,
    vertex_attributes: Arc<BTreeMap<String, AttributeColumn>>,
    prim_attributes: Arc<BTreeMap<String, AttributeColumn>>,
    detail_attributes: Arc<BTreeMap<String, AttributeColumn>>,
    point_groups: Arc<BTreeMap<String, ElementGroup>>,
    prim_groups: Arc<BTreeMap<String, ElementGroup>>,
    packed: Option<Arc<PackedTable>>,
    volumes: Option<Arc<[VolumeEntry]>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeoError {
    AttributeLengthMismatch { expected: usize, actual: usize },
    AttributeNotFound,
    AttributeTypeMismatch,
    Cancelled,
    IndexOutOfBounds,
    InvalidAttributeName,
    InvalidTopology,
    LimitExceeded,
    NonFinite,
    ReservedAttributeName,
}

impl From<MathError> for GeoError {
    fn from(_: MathError) -> Self {
        Self::NonFinite
    }
}

pub trait HashableValue: Copy {
    const TYPE_TAG: u8;
    fn update_hash(&self, hasher: &mut blake3::Hasher);
}

impl HashableValue for FiniteF32 {
    const TYPE_TAG: u8 = 1;
    fn update_hash(&self, hasher: &mut blake3::Hasher) {
        hasher.update(&self.0.to_bits().to_le_bytes());
    }
}

impl HashableValue for i32 {
    const TYPE_TAG: u8 = 2;
    fn update_hash(&self, hasher: &mut blake3::Hasher) {
        hasher.update(&self.to_le_bytes());
    }
}

impl HashableValue for u32 {
    const TYPE_TAG: u8 = 3;
    fn update_hash(&self, hasher: &mut blake3::Hasher) {
        hasher.update(&self.to_le_bytes());
    }
}

impl HashableValue for bool {
    const TYPE_TAG: u8 = 4;
    fn update_hash(&self, hasher: &mut blake3::Hasher) {
        hasher.update(&[u8::from(*self)]);
    }
}

impl HashableValue for Vec3 {
    const TYPE_TAG: u8 = 5;
    fn update_hash(&self, hasher: &mut blake3::Hasher) {
        for component in [self.x, self.y, self.z] {
            hasher.update(&component.to_bits().to_le_bytes());
        }
    }
}

impl<T: HashableValue> Page<T> {
    fn from_slice(values: &[T]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&[T::TYPE_TAG]);
        hasher.update(&(values.len() as u64).to_le_bytes());
        for value in values {
            value.update_hash(&mut hasher);
        }
        Self {
            values: Arc::from(values),
            hash: hasher.finalize(),
        }
    }

    fn from_vec(values: Vec<T>) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&[T::TYPE_TAG]);
        hasher.update(&(values.len() as u64).to_le_bytes());
        for value in &values {
            value.update_hash(&mut hasher);
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

impl<T: HashableValue> PagedColumn<T> {
    #[must_use]
    pub fn from_vec(values: Vec<T>) -> Self {
        let len = values.len();
        let pages = values
            .chunks(PAGE_LEN)
            .map(Page::from_slice)
            .collect::<Vec<_>>();
        Self {
            pages: Arc::from(pages),
            len,
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<T> {
        let page = self.pages.get(index / PAGE_LEN)?;
        page.values.get(index % PAGE_LEN).copied()
    }

    /// Produces a new column while copying only the 16K page containing `index`.
    pub fn with_element(&self, index: usize, value: T) -> Result<Self, GeoError> {
        if index >= self.len {
            return Err(GeoError::IndexOutOfBounds);
        }
        let page_index = index / PAGE_LEN;
        let mut pages = self.pages.to_vec();
        let mut values = pages[page_index].values.to_vec();
        values[index % PAGE_LEN] = value;
        pages[page_index] = Page::from_vec(values);
        Ok(Self {
            pages: Arc::from(pages),
            len: self.len,
        })
    }

    #[must_use]
    pub fn hash(&self) -> blake3::Hash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&[T::TYPE_TAG]);
        hasher.update(&(self.len as u64).to_le_bytes());
        for page in &*self.pages {
            hasher.update(page.hash.as_bytes());
        }
        hasher.finalize()
    }

    #[must_use]
    pub fn page_shared_with(&self, other: &Self, page: usize) -> bool {
        self.pages.get(page).is_some_and(|a| {
            other
                .pages
                .get(page)
                .is_some_and(|b| Arc::ptr_eq(&a.values, &b.values))
        })
    }
}

impl AttributeColumn {
    pub fn f32_checked(values: Vec<f32>) -> Result<Self, GeoError> {
        let validated = values
            .into_iter()
            .map(FiniteF32::checked)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::F32(PagedColumn::from_vec(validated)))
    }

    #[must_use]
    pub fn i32_values(values: Vec<i32>) -> Self {
        Self::I32(PagedColumn::from_vec(values))
    }

    #[must_use]
    pub fn u32_values(values: Vec<u32>) -> Self {
        Self::U32(PagedColumn::from_vec(values))
    }

    #[must_use]
    pub fn bool_values(values: Vec<bool>) -> Self {
        Self::Bool(PagedColumn::from_vec(values))
    }

    #[must_use]
    pub fn vec3_values(values: Vec<Vec3>) -> Self {
        Self::Vec3(PagedColumn::from_vec(values))
    }

    #[must_use]
    pub const fn attr_type(&self) -> AttrType {
        match self {
            Self::F32(_) => AttrType::F32,
            Self::I32(_) => AttrType::I32,
            Self::U32(_) => AttrType::U32,
            Self::Bool(_) => AttrType::Bool,
            Self::Vec3(_) => AttrType::Vec3,
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        match self {
            Self::F32(col) => col.len(),
            Self::I32(col) => col.len(),
            Self::U32(col) => col.len(),
            Self::Bool(col) => col.len(),
            Self::Vec3(col) => col.len(),
        }
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn hash(&self) -> blake3::Hash {
        match self {
            Self::F32(col) => col.hash(),
            Self::I32(col) => col.hash(),
            Self::U32(col) => col.hash(),
            Self::Bool(col) => col.hash(),
            Self::Vec3(col) => col.hash(),
        }
    }

    /// Joins two columns end to end, filling a missing side with zeros. Both sides must have the
    /// same type where both exist.
    fn concatenated(
        first: Option<&Self>,
        first_len: usize,
        second: Option<&Self>,
        second_len: usize,
    ) -> Result<Self, GeoError> {
        Self::concatenated_all(&[(first, first_len), (second, second_len)])
    }

    /// Joins any number of `(column, length)` parts end to end, filling a missing part with zeros.
    pub(crate) fn concatenated_all(parts: &[(Option<&Self>, usize)]) -> Result<Self, GeoError> {
        fn join<T: HashableValue>(
            parts: &[(Option<&AttributeColumn>, usize)],
            view: fn(&AttributeColumn) -> Option<&PagedColumn<T>>,
            zero: T,
        ) -> PagedColumn<T> {
            let total = parts.iter().map(|part| part.1).sum();
            let mut values = Vec::with_capacity(total);
            for (column, len) in parts {
                let column = column.and_then(view);
                values.extend((0..*len).map(|i| column.and_then(|c| c.get(i)).unwrap_or(zero)));
            }
            PagedColumn::from_vec(values)
        }
        let kind = parts
            .iter()
            .find_map(|part| part.0)
            .ok_or(GeoError::AttributeNotFound)?
            .attr_type();
        if parts
            .iter()
            .any(|part| part.0.is_some_and(|c| c.attr_type() != kind))
        {
            return Err(GeoError::AttributeTypeMismatch);
        }
        Ok(match kind {
            AttrType::F32 => Self::F32(join(parts, Self::as_f32, FiniteF32::ZERO)),
            AttrType::I32 => Self::I32(join(parts, Self::as_i32, 0)),
            AttrType::U32 => Self::U32(join(parts, Self::as_u32, 0)),
            AttrType::Bool => Self::Bool(join(parts, Self::as_bool, false)),
            AttrType::Vec3 => Self::Vec3(join(parts, Self::as_vec3, Vec3::ZERO)),
        })
    }

    /// Builds a new column holding `indices` of this one, in order.
    pub fn selected(&self, indices: &[usize]) -> Result<Self, GeoError> {
        fn pick<T: HashableValue>(
            column: &PagedColumn<T>,
            indices: &[usize],
        ) -> Result<Vec<T>, GeoError> {
            indices
                .iter()
                .map(|&index| column.get(index).ok_or(GeoError::IndexOutOfBounds))
                .collect()
        }
        Ok(match self {
            Self::F32(column) => Self::F32(PagedColumn::from_vec(pick(column, indices)?)),
            Self::I32(column) => Self::I32(PagedColumn::from_vec(pick(column, indices)?)),
            Self::U32(column) => Self::U32(PagedColumn::from_vec(pick(column, indices)?)),
            Self::Bool(column) => Self::Bool(PagedColumn::from_vec(pick(column, indices)?)),
            Self::Vec3(column) => Self::Vec3(PagedColumn::from_vec(pick(column, indices)?)),
        })
    }

    #[must_use]
    pub const fn as_f32(&self) -> Option<&PagedColumn<FiniteF32>> {
        match self {
            Self::F32(col) => Some(col),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_i32(&self) -> Option<&PagedColumn<i32>> {
        match self {
            Self::I32(col) => Some(col),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_u32(&self) -> Option<&PagedColumn<u32>> {
        match self {
            Self::U32(col) => Some(col),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_bool(&self) -> Option<&PagedColumn<bool>> {
        match self {
            Self::Bool(col) => Some(col),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_vec3(&self) -> Option<&PagedColumn<Vec3>> {
        match self {
            Self::Vec3(col) => Some(col),
            _ => None,
        }
    }

    #[must_use]
    pub fn page_shared_with(&self, other: &Self, page: usize) -> bool {
        match (self, other) {
            (Self::F32(a), Self::F32(b)) => a.page_shared_with(b, page),
            (Self::I32(a), Self::I32(b)) => a.page_shared_with(b, page),
            (Self::U32(a), Self::U32(b)) => a.page_shared_with(b, page),
            (Self::Bool(a), Self::Bool(b)) => a.page_shared_with(b, page),
            (Self::Vec3(a), Self::Vec3(b)) => a.page_shared_with(b, page),
            _ => false,
        }
    }
}

fn validate_attr_name(name: &str) -> Result<(), GeoError> {
    if name.is_empty() {
        return Err(GeoError::InvalidAttributeName);
    }
    if name == "P" {
        return Err(GeoError::ReservedAttributeName);
    }
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        return Err(GeoError::InvalidAttributeName);
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(GeoError::InvalidAttributeName);
    }
    Ok(())
}

impl GeoDetail {
    #[must_use]
    pub fn points(points: Vec<Vec3>) -> Self {
        Self {
            positions: PagedColumn::from_vec(points),
            vertex_points: PagedColumn::from_vec(Vec::new()),
            prim_runs: Arc::from([]),
            prim_count: 0,
            point_attributes: Arc::new(BTreeMap::new()),
            vertex_attributes: Arc::new(BTreeMap::new()),
            prim_attributes: Arc::new(BTreeMap::new()),
            detail_attributes: Arc::new(BTreeMap::new()),
            point_groups: Arc::new(BTreeMap::new()),
            prim_groups: Arc::new(BTreeMap::new()),
            packed: None,
            volumes: None,
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.positions.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    #[must_use]
    pub const fn vertex_count(&self) -> usize {
        self.vertex_points.len()
    }

    #[must_use]
    pub const fn prim_count(&self) -> usize {
        self.prim_count
    }

    #[must_use]
    pub fn prim_runs(&self) -> &[PrimRun] {
        &self.prim_runs
    }

    #[must_use]
    pub fn vertex_point(&self, vertex_index: usize) -> Option<u32> {
        self.vertex_points.get(vertex_index)
    }

    #[must_use]
    pub fn point(&self, index: usize) -> Option<Vec3> {
        self.positions.get(index)
    }

    #[must_use]
    pub const fn positions(&self) -> &PagedColumn<Vec3> {
        &self.positions
    }

    #[must_use]
    pub const fn domain_len(&self, domain: AttrDomain) -> usize {
        match domain {
            AttrDomain::Point => self.len(),
            AttrDomain::Vertex => self.vertex_count(),
            AttrDomain::Prim => self.prim_count(),
            AttrDomain::Detail => 1,
        }
    }

    #[must_use]
    pub fn attribute_names(&self) -> Vec<&str> {
        self.domain_attribute_names(AttrDomain::Point)
    }

    #[must_use]
    pub fn domain_attribute_names(&self, domain: AttrDomain) -> Vec<&str> {
        let map = match domain {
            AttrDomain::Point => &self.point_attributes,
            AttrDomain::Vertex => &self.vertex_attributes,
            AttrDomain::Prim => &self.prim_attributes,
            AttrDomain::Detail => &self.detail_attributes,
        };
        map.keys().map(String::as_str).collect()
    }

    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<&AttributeColumn> {
        self.domain_attribute(AttrDomain::Point, name)
    }

    #[must_use]
    pub fn domain_attribute(&self, domain: AttrDomain, name: &str) -> Option<&AttributeColumn> {
        match domain {
            AttrDomain::Point => self.point_attributes.get(name),
            AttrDomain::Vertex => self.vertex_attributes.get(name),
            AttrDomain::Prim => self.prim_attributes.get(name),
            AttrDomain::Detail => self.detail_attributes.get(name),
        }
    }

    /// Attaches validated primitive topology (`runs` and `vertex_points`), preserving CoW sharing
    /// of existing point positions, point attributes, and detail attributes.
    pub fn with_primitives(
        &self,
        runs: Vec<PrimRun>,
        vertex_points: Vec<u32>,
    ) -> Result<Self, GeoError> {
        // These need table entries, which only the packed and volume APIs supply.
        if runs
            .iter()
            .any(|run| matches!(run.kind, PrimKind::Packed | PrimKind::Volume))
        {
            return Err(GeoError::InvalidTopology);
        }
        self.with_runs(runs, vertex_points)
    }

    /// Attaches topology of any kind, without packed or volume data. Callers supply that
    /// separately.
    pub fn with_runs(&self, runs: Vec<PrimRun>, vertex_points: Vec<u32>) -> Result<Self, GeoError> {
        let mut expected_vertices = 0usize;
        let mut total_prims = 0usize;
        for run in &runs {
            if run.prim_count == 0 {
                return Err(GeoError::InvalidTopology);
            }
            let min_arity = match run.kind {
                PrimKind::Polygon => 3,
                PrimKind::Polyline => 2,
                PrimKind::Packed | PrimKind::Volume => 1,
                PrimKind::Tetrahedron => 4,
            };
            let exact = matches!(
                run.kind,
                PrimKind::Packed | PrimKind::Tetrahedron | PrimKind::Volume
            );
            if run.arity < min_arity || (exact && run.arity != min_arity) {
                return Err(GeoError::InvalidTopology);
            }
            let run_verts = (run.arity as usize)
                .checked_mul(run.prim_count)
                .ok_or(GeoError::LimitExceeded)?;
            expected_vertices = expected_vertices
                .checked_add(run_verts)
                .ok_or(GeoError::LimitExceeded)?;
            total_prims = total_prims
                .checked_add(run.prim_count)
                .ok_or(GeoError::LimitExceeded)?;
        }
        if vertex_points.len() != expected_vertices {
            return Err(GeoError::InvalidTopology);
        }
        let point_len = self.len();
        for &pt_idx in &vertex_points {
            if (pt_idx as usize) >= point_len {
                return Err(GeoError::IndexOutOfBounds);
            }
        }
        let mut next = self.clone();
        next.vertex_points = PagedColumn::from_vec(vertex_points);
        next.prim_runs = Arc::from(runs);
        next.prim_count = total_prims;
        next.vertex_attributes = Arc::new(BTreeMap::new());
        next.prim_attributes = Arc::new(BTreeMap::new());
        next.prim_groups = Arc::new(BTreeMap::new());
        next.packed = None;
        next.volumes = None;
        Ok(next)
    }

    /// Returns a new `GeoDetail` with the validated point attribute column inserted or replaced.
    /// Existing position pages and all other attribute columns remain CoW-shared.
    pub fn with_attribute(
        &self,
        name: impl Into<String>,
        column: AttributeColumn,
    ) -> Result<Self, GeoError> {
        self.with_domain_attribute(AttrDomain::Point, name, column)
    }

    /// Returns a new `GeoDetail` with a validated attribute column inserted or replaced on `domain`.
    pub fn with_domain_attribute(
        &self,
        domain: AttrDomain,
        name: impl Into<String>,
        column: AttributeColumn,
    ) -> Result<Self, GeoError> {
        let name = name.into();
        validate_attr_name(&name)?;
        let expected = self.domain_len(domain);
        if column.len() != expected {
            return Err(GeoError::AttributeLengthMismatch {
                expected,
                actual: column.len(),
            });
        }
        let mut next = self.clone();
        match domain {
            AttrDomain::Point => {
                let mut attrs = (*self.point_attributes).clone();
                attrs.insert(name, column);
                next.point_attributes = Arc::new(attrs);
            }
            AttrDomain::Vertex => {
                let mut attrs = (*self.vertex_attributes).clone();
                attrs.insert(name, column);
                next.vertex_attributes = Arc::new(attrs);
            }
            AttrDomain::Prim => {
                let mut attrs = (*self.prim_attributes).clone();
                attrs.insert(name, column);
                next.prim_attributes = Arc::new(attrs);
            }
            AttrDomain::Detail => {
                let mut attrs = (*self.detail_attributes).clone();
                attrs.insert(name, column);
                next.detail_attributes = Arc::new(attrs);
            }
        }
        Ok(next)
    }

    #[must_use]
    pub fn group_names(&self, domain: GroupDomain) -> Vec<&str> {
        let map = match domain {
            GroupDomain::Point => &self.point_groups,
            GroupDomain::Prim => &self.prim_groups,
        };
        map.keys().map(String::as_str).collect()
    }

    #[must_use]
    pub fn group(&self, domain: GroupDomain, name: &str) -> Option<&ElementGroup> {
        match domain {
            GroupDomain::Point => self.point_groups.get(name),
            GroupDomain::Prim => self.prim_groups.get(name),
        }
    }

    pub fn with_group(
        &self,
        domain: GroupDomain,
        name: impl Into<String>,
        selected_indices: &[usize],
    ) -> Result<Self, GeoError> {
        let name = name.into();
        validate_attr_name(&name)?;
        let len = match domain {
            GroupDomain::Point => self.len(),
            GroupDomain::Prim => self.prim_count(),
        };
        let mut mask = vec![false; len];
        let mut selected_count = 0usize;
        for &idx in selected_indices {
            let slot = mask.get_mut(idx).ok_or(GeoError::IndexOutOfBounds)?;
            if !*slot {
                *slot = true;
                selected_count += 1;
            }
        }
        let group = ElementGroup {
            mask: PagedColumn::from_vec(mask),
            selected_count,
        };
        let mut next = self.clone();
        match domain {
            GroupDomain::Point => {
                let mut groups = (*self.point_groups).clone();
                groups.insert(name, group);
                next.point_groups = Arc::new(groups);
            }
            GroupDomain::Prim => {
                let mut groups = (*self.prim_groups).clone();
                groups.insert(name, group);
                next.prim_groups = Arc::new(groups);
            }
        }
        Ok(next)
    }

    /// Updates a single `f32` point attribute element at `index`, copying only the affected 16K page
    /// of that attribute column while sharing all other pages and columns.
    pub fn with_f32_attribute_element(
        &self,
        name: &str,
        index: usize,
        value: f32,
    ) -> Result<Self, GeoError> {
        let checked = FiniteF32::checked(value)?;
        let current = self
            .point_attributes
            .get(name)
            .ok_or(GeoError::AttributeNotFound)?;
        let col = current.as_f32().ok_or(GeoError::AttributeTypeMismatch)?;
        let updated_col = col.with_element(index, checked)?;
        let mut attrs = (*self.point_attributes).clone();
        attrs.insert(name.to_owned(), AttributeColumn::F32(updated_col));
        let mut next = self.clone();
        next.point_attributes = Arc::new(attrs);
        Ok(next)
    }

    /// Updates a single `Vec3` point attribute element at `index`, copying only the affected 16K page
    /// of that attribute column while sharing all other pages and columns.
    pub fn with_vec3_attribute_element(
        &self,
        name: &str,
        index: usize,
        value: Vec3,
    ) -> Result<Self, GeoError> {
        let current = self
            .point_attributes
            .get(name)
            .ok_or(GeoError::AttributeNotFound)?;
        let col = current.as_vec3().ok_or(GeoError::AttributeTypeMismatch)?;
        let updated_col = col.with_element(index, value)?;
        let mut attrs = (*self.point_attributes).clone();
        attrs.insert(name.to_owned(), AttributeColumn::Vec3(updated_col));
        let mut next = self.clone();
        next.point_attributes = Arc::new(attrs);
        Ok(next)
    }

    pub fn translated(&self, offset: Vec3, memory_limit: usize) -> Result<Self, GeoError> {
        let paused = AtomicBool::new(false);
        self.translated_cancellable(offset, memory_limit, &paused)
    }

    pub fn merged(&self, other: &Self, memory_limit: usize) -> Result<Self, GeoError> {
        let paused = AtomicBool::new(false);
        self.merged_cancellable(other, memory_limit, &paused)
    }

    pub fn merged_cancellable(
        &self,
        other: &Self,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        let self_len = self.len();
        let total_len = self_len
            .checked_add(other.len())
            .ok_or(GeoError::LimitExceeded)?;
        let bytes = total_len
            .checked_mul(std::mem::size_of::<Vec3>())
            .ok_or(GeoError::LimitExceeded)?;
        if bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }
        let mut points = Vec::with_capacity(total_len);
        for page in &*self.positions.pages {
            if cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            points.extend_from_slice(&page.values);
        }
        for page in &*other.positions.pages {
            if cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            points.extend_from_slice(&page.values);
        }
        let base = Self::points(points);
        if self.prim_runs.is_empty() && other.prim_runs.is_empty() {
            return base.with_merged_data(self, other);
        }
        let mut runs = Vec::with_capacity(self.prim_runs.len() + other.prim_runs.len());
        runs.extend_from_slice(&self.prim_runs);
        for run in &*other.prim_runs {
            if let Some(last) = runs.last_mut()
                && last.kind == run.kind
                && last.arity == run.arity
            {
                last.prim_count = last
                    .prim_count
                    .checked_add(run.prim_count)
                    .ok_or(GeoError::LimitExceeded)?;
            } else {
                runs.push(*run);
            }
        }
        let total_vtx = self
            .vertex_count()
            .checked_add(other.vertex_count())
            .ok_or(GeoError::LimitExceeded)?;
        let mut vertex_points = Vec::with_capacity(total_vtx);
        for page in &*self.vertex_points.pages {
            vertex_points.extend_from_slice(&page.values);
        }
        let pt_offset = u32::try_from(self_len).map_err(|_| GeoError::LimitExceeded)?;
        for page in &*other.vertex_points.pages {
            for &pt in &*page.values {
                let shifted = pt.checked_add(pt_offset).ok_or(GeoError::LimitExceeded)?;
                vertex_points.push(shifted);
            }
        }
        base.with_runs(runs, vertex_points)?
            .with_merged_data(self, other)
    }

    /// Fills in attributes and groups after a merge. Elements from the side lacking an attribute
    /// read as zero and elements of a side lacking a group are unselected, while detail values come
    /// from the first side that has them. An attribute must have one type on both sides.
    fn with_merged_data(mut self, first: &Self, second: &Self) -> Result<Self, GeoError> {
        let columns = |left: &BTreeMap<String, AttributeColumn>,
                       right: &BTreeMap<String, AttributeColumn>,
                       left_len: usize,
                       right_len: usize|
         -> Result<BTreeMap<String, AttributeColumn>, GeoError> {
            let mut merged = BTreeMap::new();
            for name in left.keys().chain(right.keys()) {
                if !merged.contains_key(name) {
                    let column = AttributeColumn::concatenated(
                        left.get(name),
                        left_len,
                        right.get(name),
                        right_len,
                    )?;
                    merged.insert(name.clone(), column);
                }
            }
            Ok(merged)
        };
        self.point_attributes = Arc::new(columns(
            &first.point_attributes,
            &second.point_attributes,
            first.len(),
            second.len(),
        )?);
        self.vertex_attributes = Arc::new(columns(
            &first.vertex_attributes,
            &second.vertex_attributes,
            first.vertex_count(),
            second.vertex_count(),
        )?);
        self.prim_attributes = Arc::new(columns(
            &first.prim_attributes,
            &second.prim_attributes,
            first.prim_count(),
            second.prim_count(),
        )?);
        let mut detail = (*first.detail_attributes).clone();
        for (name, column) in &*second.detail_attributes {
            detail.entry(name.clone()).or_insert_with(|| column.clone());
        }
        self.detail_attributes = Arc::new(detail);
        self.packed =
            PackedTable::merged(first.packed.as_deref(), second.packed.as_deref()).map(Arc::new);
        self.volumes = Self::merged_volumes(first, second);
        let groups = |left: &BTreeMap<String, ElementGroup>,
                      right: &BTreeMap<String, ElementGroup>,
                      left_len: usize,
                      right_len: usize| {
            let mut merged = BTreeMap::new();
            for name in left.keys().chain(right.keys()) {
                if merged.contains_key(name) {
                    continue;
                }
                let selected = |group: Option<&ElementGroup>, index: usize| {
                    group.is_some_and(|found| found.contains(index))
                };
                let mut mask = Vec::with_capacity(left_len + right_len);
                mask.extend((0..left_len).map(|i| selected(left.get(name), i)));
                mask.extend((0..right_len).map(|i| selected(right.get(name), i)));
                let selected_count = mask.iter().filter(|&&selected| selected).count();
                merged.insert(
                    name.clone(),
                    ElementGroup {
                        mask: PagedColumn::from_vec(mask),
                        selected_count,
                    },
                );
            }
            merged
        };
        self.point_groups = Arc::new(groups(
            &first.point_groups,
            &second.point_groups,
            first.len(),
            second.len(),
        ));
        self.prim_groups = Arc::new(groups(
            &first.prim_groups,
            &second.prim_groups,
            first.prim_count(),
            second.prim_count(),
        ));
        Ok(self)
    }

    #[must_use]
    pub fn topology_index(&self) -> TopologyIndex {
        TopologyIndex::build(self)
    }

    /// Computes total polygon surface area (sum of triangle fan areas across all `PrimKind::Polygon` faces).
    #[must_use]
    pub fn surface_area(&self) -> f32 {
        let mut area = 0.0f32;
        let mut vtx_cursor = 0usize;
        for run in &*self.prim_runs {
            let arity = run.arity as usize;
            if run.kind == PrimKind::Polygon && arity >= 3 {
                for _ in 0..run.prim_count {
                    if let Some(p0_idx) = self.vertex_point(vtx_cursor)
                        && let Some(p0) = self.point(p0_idx as usize)
                    {
                        for k in 1..(arity - 1) {
                            if let (Some(pk_idx), Some(pk1_idx)) = (
                                self.vertex_point(vtx_cursor + k),
                                self.vertex_point(vtx_cursor + k + 1),
                            ) && let (Some(pk), Some(pk1)) =
                                (self.point(pk_idx as usize), self.point(pk1_idx as usize))
                                && let (Ok(e1), Ok(e2)) = (pk.sub_checked(p0), pk1.sub_checked(p0))
                                && let Ok(cross) = e1.cross_checked(e2)
                            {
                                area += 0.5 * cross.length();
                            }
                        }
                    }
                    vtx_cursor += arity;
                }
            } else {
                vtx_cursor += arity * run.prim_count;
            }
        }
        area
    }

    /// Computes enclosed volume of polygon faces using the Divergence Theorem:
    /// $V = \left|\frac{1}{6} \sum \mathbf{p}_0 \cdot (\mathbf{p}_k \times \mathbf{p}_{k+1})\right|$.
    #[must_use]
    pub fn enclosed_volume(&self) -> f32 {
        let mut signed_six_vol = 0.0f32;
        let mut vtx_cursor = 0usize;
        for run in &*self.prim_runs {
            let arity = run.arity as usize;
            if run.kind == PrimKind::Polygon && arity >= 3 {
                for _ in 0..run.prim_count {
                    if let Some(p0_idx) = self.vertex_point(vtx_cursor)
                        && let Some(p0) = self.point(p0_idx as usize)
                    {
                        for k in 1..(arity - 1) {
                            if let (Some(pk_idx), Some(pk1_idx)) = (
                                self.vertex_point(vtx_cursor + k),
                                self.vertex_point(vtx_cursor + k + 1),
                            ) && let (Some(pk), Some(pk1)) =
                                (self.point(pk_idx as usize), self.point(pk1_idx as usize))
                                && let Ok(cross) = pk.cross_checked(pk1)
                            {
                                signed_six_vol += p0.dot(cross);
                            }
                        }
                    }
                    vtx_cursor += arity;
                }
            } else {
                vtx_cursor += arity * run.prim_count;
            }
        }
        (signed_six_vol / 6.0).abs()
    }

    pub fn compute_normals(&self, memory_limit: usize) -> Result<Self, GeoError> {
        let paused = AtomicBool::new(false);
        self.compute_normals_cancellable(memory_limit, &paused)
    }

    /// Computes area-weighted point normals (`v@N`) across all polygon faces,
    /// defaulting isolated points to `Vec3::UP` (`(0, 1, 0)`).
    pub fn compute_normals_cancellable(
        &self,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        let pt_len = self.len();
        let bytes = pt_len
            .checked_mul(std::mem::size_of::<Vec3>())
            .ok_or(GeoError::LimitExceeded)?;
        if bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }
        let mut accum = vec![Vec3::ZERO; pt_len];
        let mut vtx_cursor = 0usize;
        for run in &*self.prim_runs {
            let arity = run.arity as usize;
            if run.kind == PrimKind::Polygon && arity >= 3 {
                for prim_i in 0..run.prim_count {
                    if prim_i.is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                        return Err(GeoError::Cancelled);
                    }
                    let mut face_normal = Vec3::ZERO;
                    if let Some(p0_idx) = self.vertex_point(vtx_cursor)
                        && let Some(p0) = self.point(p0_idx as usize)
                    {
                        for k in 1..(arity - 1) {
                            if let (Some(pk_idx), Some(pk1_idx)) = (
                                self.vertex_point(vtx_cursor + k),
                                self.vertex_point(vtx_cursor + k + 1),
                            ) && let (Some(pk), Some(pk1)) =
                                (self.point(pk_idx as usize), self.point(pk1_idx as usize))
                                && let (Ok(e1), Ok(e2)) = (pk.sub_checked(p0), pk1.sub_checked(p0))
                                && let Ok(cross) = e1.cross_checked(e2)
                                && let Ok(sum) = face_normal.add_checked(cross)
                            {
                                face_normal = sum;
                            }
                        }
                    }
                    for k in 0..arity {
                        if let Some(pt_idx) = self.vertex_point(vtx_cursor + k)
                            && let Some(slot) = accum.get_mut(pt_idx as usize)
                            && let Ok(sum) = slot.add_checked(face_normal)
                        {
                            *slot = sum;
                        }
                    }
                    vtx_cursor += arity;
                }
            } else {
                vtx_cursor += arity * run.prim_count;
            }
        }
        let normals = accum
            .into_iter()
            .map(Vec3::normalized_or_up)
            .collect::<Vec<_>>();
        self.with_attribute("N", AttributeColumn::vec3_values(normals))
    }

    pub fn subdivide(&self, iterations: u32, memory_limit: usize) -> Result<Self, GeoError> {
        let paused = AtomicBool::new(false);
        self.subdivide_cancellable(iterations, memory_limit, &paused)
    }

    /// Subdivides a manifold polygon mesh using Catmull-Clark subdivision (`iterations` steps).
    /// Handles both interior manifold edges (via `TopologyIndex::twin`) and boundary edges,
    /// outputting an all-quad `GeoDetail` that preserves `is_closed_manifold()` on closed meshes.
    pub fn subdivide_cancellable(
        &self,
        iterations: u32,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        if iterations == 0 || self.prim_count() == 0 {
            return Ok(self.clone());
        }
        let mut current = self.clone();
        for _ in 0..iterations {
            if cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            for run in &*current.prim_runs {
                if run.kind != PrimKind::Polygon || run.arity < 3 {
                    return Err(GeoError::InvalidTopology);
                }
            }
            let topo = current.topology_index();
            if !topo.is_manifold() {
                return Err(GeoError::InvalidTopology);
            }
            let orig_pt_count = current.len();
            let face_count = current.prim_count();
            let vtx_count = current.vertex_count();

            // Build unique undirected edges from half-edges: (u_pt, v_pt, he0, opt_he1)
            let mut he_to_edge = vec![0usize; vtx_count];
            let mut edges: Vec<(u32, u32, u32, Option<u32>)> = Vec::with_capacity(vtx_count);
            for he in 0..(vtx_count as u32) {
                if let Some(tw) = topo.twin(he) {
                    if he < tw {
                        let edge_idx = edges.len();
                        let nxt = topo.next(he).ok_or(GeoError::InvalidTopology)?;
                        let u = current
                            .vertex_point(he as usize)
                            .ok_or(GeoError::InvalidTopology)?;
                        let v = current
                            .vertex_point(nxt as usize)
                            .ok_or(GeoError::InvalidTopology)?;
                        edges.push((u, v, he, Some(tw)));
                        he_to_edge[he as usize] = edge_idx;
                        he_to_edge[tw as usize] = edge_idx;
                    }
                } else {
                    let edge_idx = edges.len();
                    let nxt = topo.next(he).ok_or(GeoError::InvalidTopology)?;
                    let u = current
                        .vertex_point(he as usize)
                        .ok_or(GeoError::InvalidTopology)?;
                    let v = current
                        .vertex_point(nxt as usize)
                        .ok_or(GeoError::InvalidTopology)?;
                    edges.push((u, v, he, None));
                    he_to_edge[he as usize] = edge_idx;
                }
            }
            let edge_count = edges.len();
            let new_pt_count = orig_pt_count
                .checked_add(face_count)
                .and_then(|n| n.checked_add(edge_count))
                .ok_or(GeoError::LimitExceeded)?;
            let _ = u32::try_from(new_pt_count).map_err(|_| GeoError::LimitExceeded)?;
            let new_face_count = vtx_count;
            let new_vtx_count = new_face_count
                .checked_mul(4)
                .ok_or(GeoError::LimitExceeded)?;
            let pt_bytes = new_pt_count
                .checked_mul(std::mem::size_of::<Vec3>())
                .ok_or(GeoError::LimitExceeded)?;
            let vtx_bytes = new_vtx_count
                .checked_mul(std::mem::size_of::<u32>())
                .ok_or(GeoError::LimitExceeded)?;
            if pt_bytes
                .checked_add(vtx_bytes)
                .ok_or(GeoError::LimitExceeded)?
                > memory_limit
            {
                return Err(GeoError::LimitExceeded);
            }

            // 1. Face points (centroid of each polygon face)
            let mut face_points = Vec::with_capacity(face_count);
            for f in 0..face_count {
                if f.is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                    return Err(GeoError::Cancelled);
                }
                let start = topo
                    .prim_vertex_start(f as u32)
                    .ok_or(GeoError::InvalidTopology)? as usize;
                let arity = topo.prim_arity(f as u32).ok_or(GeoError::InvalidTopology)? as usize;
                let mut sum = Vec3::ZERO;
                for k in 0..arity {
                    let pt_idx = current
                        .vertex_point(start + k)
                        .ok_or(GeoError::InvalidTopology)?
                        as usize;
                    let pt = current.point(pt_idx).ok_or(GeoError::IndexOutOfBounds)?;
                    sum = sum.add_checked(pt)?;
                }
                face_points.push(sum.scale_checked(1.0 / (arity as f32))?);
            }

            // 2. Edge midpoints and Catmull-Clark edge points
            let mut edge_midpoints = Vec::with_capacity(edge_count);
            let mut edge_points = Vec::with_capacity(edge_count);
            for (e_idx, &(u, v, he0, opt_he1)) in edges.iter().enumerate() {
                if e_idx.is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                    return Err(GeoError::Cancelled);
                }
                let pu = current
                    .point(u as usize)
                    .ok_or(GeoError::IndexOutOfBounds)?;
                let pv = current
                    .point(v as usize)
                    .ok_or(GeoError::IndexOutOfBounds)?;
                let mid = pu.add_checked(pv)?.scale_checked(0.5)?;
                edge_midpoints.push(mid);
                if let Some(he1) = opt_he1 {
                    let f0 = topo.prim_of_vertex(he0).ok_or(GeoError::InvalidTopology)? as usize;
                    let f1 = topo.prim_of_vertex(he1).ok_or(GeoError::InvalidTopology)? as usize;
                    let ep = pu
                        .add_checked(pv)?
                        .add_checked(face_points[f0])?
                        .add_checked(face_points[f1])?
                        .scale_checked(0.25)?;
                    edge_points.push(ep);
                } else {
                    edge_points.push(mid);
                }
            }

            // 3. Updated original vertex positions
            let mut face_sum = vec![Vec3::ZERO; orig_pt_count];
            let mut face_deg = vec![0u32; orig_pt_count];
            for (f, &fp) in face_points.iter().enumerate() {
                let start = topo
                    .prim_vertex_start(f as u32)
                    .ok_or(GeoError::InvalidTopology)? as usize;
                let arity = topo.prim_arity(f as u32).ok_or(GeoError::InvalidTopology)? as usize;
                for k in 0..arity {
                    let pt_idx = current
                        .vertex_point(start + k)
                        .ok_or(GeoError::InvalidTopology)?
                        as usize;
                    face_sum[pt_idx] = face_sum[pt_idx].add_checked(fp)?;
                    face_deg[pt_idx] += 1;
                }
            }

            let mut edge_mid_sum = vec![Vec3::ZERO; orig_pt_count];
            let mut edge_deg = vec![0u32; orig_pt_count];
            let mut boundary_mid_sum = vec![Vec3::ZERO; orig_pt_count];
            let mut boundary_deg = vec![0u32; orig_pt_count];
            for (e_idx, &(u, v, _, opt_he1)) in edges.iter().enumerate() {
                let mid = edge_midpoints[e_idx];
                for pt_idx in [u as usize, v as usize] {
                    edge_mid_sum[pt_idx] = edge_mid_sum[pt_idx].add_checked(mid)?;
                    edge_deg[pt_idx] += 1;
                    if opt_he1.is_none() {
                        boundary_mid_sum[pt_idx] = boundary_mid_sum[pt_idx].add_checked(mid)?;
                        boundary_deg[pt_idx] += 1;
                    }
                }
            }

            let mut new_points = Vec::with_capacity(new_pt_count);
            for pt_idx in 0..orig_pt_count {
                if pt_idx.is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                    return Err(GeoError::Cancelled);
                }
                let p_old = current.point(pt_idx).ok_or(GeoError::IndexOutOfBounds)?;
                let p_new = if boundary_deg[pt_idx] > 0 {
                    let avg_b = boundary_mid_sum[pt_idx]
                        .scale_checked(1.0 / (boundary_deg[pt_idx] as f32))?;
                    p_old
                        .scale_checked(0.5)?
                        .add_checked(avg_b.scale_checked(0.5)?)?
                } else if face_deg[pt_idx] > 0 && edge_deg[pt_idx] > 0 {
                    let n_faces = face_deg[pt_idx] as f32;
                    let n_edges = edge_deg[pt_idx] as f32;
                    let f_avg = face_sum[pt_idx].scale_checked(1.0 / n_faces)?;
                    let r_avg = edge_mid_sum[pt_idx].scale_checked(1.0 / n_edges)?;
                    let two_r = r_avg.scale_checked(2.0)?;
                    let p_weighted = p_old.scale_checked(n_faces - 3.0)?;
                    f_avg
                        .add_checked(two_r)?
                        .add_checked(p_weighted)?
                        .scale_checked(1.0 / n_faces)?
                } else {
                    p_old
                };
                new_points.push(p_new);
            }
            new_points.extend_from_slice(&face_points);
            new_points.extend_from_slice(&edge_points);

            // 4. Construct 4 sub-quads per original polygon corner
            let mut new_vertex_points = Vec::with_capacity(new_vtx_count);
            for f in 0..face_count {
                let start = topo
                    .prim_vertex_start(f as u32)
                    .ok_or(GeoError::InvalidTopology)? as usize;
                let arity = topo.prim_arity(f as u32).ok_or(GeoError::InvalidTopology)? as usize;
                let f_pt_idx = (orig_pt_count + f) as u32;
                for k in 0..arity {
                    let he = (start + k) as u32;
                    let prev_he = topo.prev(he).ok_or(GeoError::InvalidTopology)?;
                    let corner_pt = current
                        .vertex_point(he as usize)
                        .ok_or(GeoError::InvalidTopology)?;
                    let e_out_pt = (orig_pt_count + face_count + he_to_edge[he as usize]) as u32;
                    let e_in_pt =
                        (orig_pt_count + face_count + he_to_edge[prev_he as usize]) as u32;
                    new_vertex_points.extend_from_slice(&[corner_pt, e_out_pt, f_pt_idx, e_in_pt]);
                }
            }

            current = GeoDetail::points(new_points).with_primitives(
                vec![PrimRun {
                    kind: PrimKind::Polygon,
                    arity: 4,
                    prim_count: new_face_count,
                }],
                new_vertex_points,
            )?;
        }
        Ok(current)
    }

    pub fn poly_extrude(
        &self,
        distance: f32,
        inset: f32,
        memory_limit: usize,
    ) -> Result<Self, GeoError> {
        let paused = AtomicBool::new(false);
        self.poly_extrude_cancellable(distance, inset, memory_limit, &paused)
    }

    pub fn poly_extrude_cancellable(
        &self,
        distance: f32,
        inset: f32,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        self.poly_extrude_group_cancellable(None, distance, inset, memory_limit, cancelled)
    }

    /// Extrudes polygon faces (all faces if `group_name` is `None`, or faces in the named
    /// `GroupDomain::Prim` group) along their face normals by `distance` with optional `inset`,
    /// stitching side-wall quads around each extruded face so closed manifolds remain closed manifolds.
    pub fn poly_extrude_group_cancellable(
        &self,
        group_name: Option<&str>,
        distance: f32,
        inset: f32,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        if !distance.is_finite() || !inset.is_finite() {
            return Err(GeoError::NonFinite);
        }
        if inset < 0.0 {
            return Err(GeoError::InvalidTopology);
        }
        if self.prim_count() == 0 {
            return Err(GeoError::InvalidTopology);
        }
        for run in &*self.prim_runs {
            if run.kind != PrimKind::Polygon || run.arity < 3 {
                return Err(GeoError::InvalidTopology);
            }
        }
        let topo = self.topology_index();
        if !topo.is_manifold() {
            return Err(GeoError::InvalidTopology);
        }
        let opt_group = if let Some(gname) = group_name {
            Some(
                self.group(GroupDomain::Prim, gname)
                    .ok_or(GeoError::AttributeNotFound)?,
            )
        } else {
            None
        };

        let face_count = self.prim_count();
        let mut added_pts = 0usize;
        let mut total_out_vtx = 0usize;
        for f in 0..face_count {
            let arity = topo.prim_arity(f as u32).ok_or(GeoError::InvalidTopology)? as usize;
            let selected = opt_group.is_none_or(|g| g.contains(f));
            if selected {
                added_pts = added_pts
                    .checked_add(arity)
                    .ok_or(GeoError::LimitExceeded)?;
                let side_vtx = arity.checked_mul(4).ok_or(GeoError::LimitExceeded)?;
                total_out_vtx = total_out_vtx
                    .checked_add(arity)
                    .and_then(|n| n.checked_add(side_vtx))
                    .ok_or(GeoError::LimitExceeded)?;
            } else {
                total_out_vtx = total_out_vtx
                    .checked_add(arity)
                    .ok_or(GeoError::LimitExceeded)?;
            }
        }
        let new_pt_count = self
            .len()
            .checked_add(added_pts)
            .ok_or(GeoError::LimitExceeded)?;
        let _ = u32::try_from(new_pt_count).map_err(|_| GeoError::LimitExceeded)?;
        let pt_bytes = new_pt_count
            .checked_mul(std::mem::size_of::<Vec3>())
            .ok_or(GeoError::LimitExceeded)?;
        let vtx_bytes = total_out_vtx
            .checked_mul(std::mem::size_of::<u32>())
            .ok_or(GeoError::LimitExceeded)?;
        if pt_bytes
            .checked_add(vtx_bytes)
            .ok_or(GeoError::LimitExceeded)?
            > memory_limit
        {
            return Err(GeoError::LimitExceeded);
        }

        let mut points = Vec::with_capacity(new_pt_count);
        for page in &*self.positions.pages {
            points.extend_from_slice(&page.values);
        }
        let mut runs: Vec<PrimRun> = Vec::new();
        let mut vertex_points: Vec<u32> = Vec::with_capacity(total_out_vtx);

        for f in 0..face_count {
            if f.is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            let start = topo
                .prim_vertex_start(f as u32)
                .ok_or(GeoError::InvalidTopology)? as usize;
            let arity = topo.prim_arity(f as u32).ok_or(GeoError::InvalidTopology)? as usize;
            let selected = opt_group.is_none_or(|g| g.contains(f));
            if !selected {
                append_prim_run(&mut runs, PrimKind::Polygon, arity as u32, 1)?;
                for k in 0..arity {
                    let pt = self
                        .vertex_point(start + k)
                        .ok_or(GeoError::InvalidTopology)?;
                    vertex_points.push(pt);
                }
                continue;
            }

            // Gather original points of face `f`, centroid, and outward unit normal
            let mut orig_indices = Vec::with_capacity(arity);
            let mut orig_positions = Vec::with_capacity(arity);
            let mut centroid_sum = Vec3::ZERO;
            for k in 0..arity {
                let pt_idx = self
                    .vertex_point(start + k)
                    .ok_or(GeoError::InvalidTopology)?;
                let pos = self
                    .point(pt_idx as usize)
                    .ok_or(GeoError::IndexOutOfBounds)?;
                orig_indices.push(pt_idx);
                orig_positions.push(pos);
                centroid_sum = centroid_sum.add_checked(pos)?;
            }
            let centroid = centroid_sum.scale_checked(1.0 / (arity as f32))?;
            let p0 = orig_positions[0];
            let mut normal_accum = Vec3::ZERO;
            for k in 1..(arity - 1) {
                let e1 = orig_positions[k].sub_checked(p0)?;
                let e2 = orig_positions[k + 1].sub_checked(p0)?;
                let cross = e1.cross_checked(e2)?;
                normal_accum = normal_accum.add_checked(cross)?;
            }
            let face_normal = normal_accum.normalized_or_up();
            let offset_vec = face_normal.scale_checked(distance)?;

            let cap_base = points.len() as u32;
            for &pos in &orig_positions {
                let inset_pos = if inset > 0.0 {
                    let to_center = centroid.sub_checked(pos)?;
                    let dist_to_center = to_center.length();
                    if dist_to_center > 1e-6 {
                        let step = inset.min(dist_to_center * 0.95);
                        let dir = to_center.scale_checked(1.0 / dist_to_center)?;
                        pos.add_checked(dir.scale_checked(step)?)?
                    } else {
                        pos
                    }
                } else {
                    pos
                };
                points.push(inset_pos.add_checked(offset_vec)?);
            }

            // Emit extruded cap polygon
            append_prim_run(&mut runs, PrimKind::Polygon, arity as u32, 1)?;
            for k in 0..arity {
                vertex_points.push(cap_base + (k as u32));
            }
            // Emit `arity` side-wall quads around the perimeter of `f`
            append_prim_run(&mut runs, PrimKind::Polygon, 4, arity)?;
            for k in 0..arity {
                let next_k = (k + 1) % arity;
                let u = orig_indices[k];
                let v = orig_indices[next_k];
                let v_cap = cap_base + (next_k as u32);
                let u_cap = cap_base + (k as u32);
                vertex_points.extend_from_slice(&[u, v, v_cap, u_cap]);
            }
        }

        GeoDetail::points(points).with_primitives(runs, vertex_points)
    }

    /// Promotes an `F32` or `Vec3` attribute from `from_domain` to `to_domain` using `mode`,
    /// preserving Copy-on-Write page sharing for positions and all unaffected attribute columns.
    pub fn promote_attribute(
        &self,
        name: &str,
        from_domain: AttrDomain,
        to_domain: AttrDomain,
        mode: PromoteMode,
    ) -> Result<Self, GeoError> {
        validate_attr_name(name)?;
        let src_col = self
            .domain_attribute(from_domain, name)
            .ok_or(GeoError::AttributeNotFound)?;
        if from_domain == to_domain {
            return Ok(self.clone());
        }

        let promoted_col = match src_col {
            AttributeColumn::F32(col) => {
                let src_vals: Vec<f32> = (0..col.len())
                    .map(|i| col.get(i).map_or(0.0, FiniteF32::get))
                    .collect();
                let dst_vals = self.promote_values_f32(&src_vals, from_domain, to_domain, mode)?;
                AttributeColumn::f32_checked(dst_vals)?
            }
            AttributeColumn::Vec3(col) => {
                let src_vals: Vec<Vec3> = (0..col.len())
                    .map(|i| col.get(i).unwrap_or(Vec3::ZERO))
                    .collect();
                let dst_vals = self.promote_values_vec3(&src_vals, from_domain, to_domain, mode)?;
                AttributeColumn::vec3_values(dst_vals)
            }
            AttributeColumn::I32(_) | AttributeColumn::U32(_) | AttributeColumn::Bool(_) => {
                return Err(GeoError::AttributeTypeMismatch);
            }
        };

        let mut next = self.with_domain_attribute(to_domain, name, promoted_col)?;
        match from_domain {
            AttrDomain::Point => {
                let mut attrs = (*next.point_attributes).clone();
                attrs.remove(name);
                next.point_attributes = Arc::new(attrs);
            }
            AttrDomain::Vertex => {
                let mut attrs = (*next.vertex_attributes).clone();
                attrs.remove(name);
                next.vertex_attributes = Arc::new(attrs);
            }
            AttrDomain::Prim => {
                let mut attrs = (*next.prim_attributes).clone();
                attrs.remove(name);
                next.prim_attributes = Arc::new(attrs);
            }
            AttrDomain::Detail => {
                let mut attrs = (*next.detail_attributes).clone();
                attrs.remove(name);
                next.detail_attributes = Arc::new(attrs);
            }
        }
        Ok(next)
    }

    fn promote_values_f32(
        &self,
        src: &[f32],
        from_domain: AttrDomain,
        to_domain: AttrDomain,
        mode: PromoteMode,
    ) -> Result<Vec<f32>, GeoError> {
        let groups = self.build_promotion_index_groups(from_domain, to_domain);
        let mut out = Vec::with_capacity(groups.len());
        for indices in &groups {
            let mut vals = Vec::with_capacity(indices.len());
            for &idx in indices {
                if let Some(&v) = src.get(idx) {
                    vals.push(v);
                }
            }
            out.push(reduce_f32(&vals, mode)?);
        }
        Ok(out)
    }

    fn promote_values_vec3(
        &self,
        src: &[Vec3],
        from_domain: AttrDomain,
        to_domain: AttrDomain,
        mode: PromoteMode,
    ) -> Result<Vec<Vec3>, GeoError> {
        let groups = self.build_promotion_index_groups(from_domain, to_domain);
        let mut out = Vec::with_capacity(groups.len());
        for indices in &groups {
            let mut vals = Vec::with_capacity(indices.len());
            for &idx in indices {
                if let Some(&v) = src.get(idx) {
                    vals.push(v);
                }
            }
            out.push(reduce_vec3(&vals, mode)?);
        }
        Ok(out)
    }

    /// Maps each destination element in `to_domain` to the list of contributing source indices in `from_domain`.
    fn build_promotion_index_groups(
        &self,
        from_domain: AttrDomain,
        to_domain: AttrDomain,
    ) -> Vec<Vec<usize>> {
        let dst_len = self.domain_len(to_domain);
        let src_len = self.domain_len(from_domain);
        match (from_domain, to_domain) {
            (_, AttrDomain::Detail) => vec![(0..src_len).collect()],
            (AttrDomain::Detail, _) => vec![vec![0]; dst_len],
            (AttrDomain::Point, AttrDomain::Prim) => {
                let mut groups = Vec::with_capacity(self.prim_count());
                let mut vtx_cursor = 0usize;
                for run in &*self.prim_runs {
                    let arity = run.arity as usize;
                    for _ in 0..run.prim_count {
                        let mut pts = Vec::with_capacity(arity);
                        for k in 0..arity {
                            if let Some(pt) = self.vertex_point(vtx_cursor + k) {
                                pts.push(pt as usize);
                            }
                        }
                        groups.push(pts);
                        vtx_cursor += arity;
                    }
                }
                groups
            }
            (AttrDomain::Prim, AttrDomain::Point) => {
                let mut groups = vec![Vec::new(); self.len()];
                let mut vtx_cursor = 0usize;
                let mut prim_idx = 0usize;
                for run in &*self.prim_runs {
                    let arity = run.arity as usize;
                    for _ in 0..run.prim_count {
                        for k in 0..arity {
                            if let Some(pt) = self.vertex_point(vtx_cursor + k)
                                && let Some(list) = groups.get_mut(pt as usize)
                                && list.last().copied() != Some(prim_idx)
                            {
                                list.push(prim_idx);
                            }
                        }
                        prim_idx += 1;
                        vtx_cursor += arity;
                    }
                }
                groups
            }
            (AttrDomain::Point, AttrDomain::Vertex) => (0..self.vertex_count())
                .map(|v| {
                    self.vertex_point(v)
                        .map_or_else(Vec::new, |pt| vec![pt as usize])
                })
                .collect(),
            (AttrDomain::Vertex, AttrDomain::Point) => {
                let mut groups = vec![Vec::new(); self.len()];
                for v in 0..self.vertex_count() {
                    if let Some(pt) = self.vertex_point(v)
                        && let Some(list) = groups.get_mut(pt as usize)
                    {
                        list.push(v);
                    }
                }
                groups
            }
            (AttrDomain::Vertex, AttrDomain::Prim) => {
                let mut groups = Vec::with_capacity(self.prim_count());
                let mut vtx_cursor = 0usize;
                for run in &*self.prim_runs {
                    let arity = run.arity as usize;
                    for _ in 0..run.prim_count {
                        groups.push((vtx_cursor..(vtx_cursor + arity)).collect());
                        vtx_cursor += arity;
                    }
                }
                groups
            }
            (AttrDomain::Prim, AttrDomain::Vertex) => {
                let mut groups = Vec::with_capacity(self.vertex_count());
                let mut prim_idx = 0usize;
                for run in &*self.prim_runs {
                    let arity = run.arity as usize;
                    for _ in 0..run.prim_count {
                        for _ in 0..arity {
                            groups.push(vec![prim_idx]);
                        }
                        prim_idx += 1;
                    }
                }
                groups
            }
            _ => (0..dst_len).map(|i| vec![i]).collect(),
        }
    }

    /// Creates a `Point` or `Prim` selection group containing all elements within `[min, max]`.
    pub fn group_by_bounds(
        &self,
        domain: GroupDomain,
        name: impl Into<String>,
        min: Vec3,
        max: Vec3,
    ) -> Result<Self, GeoError> {
        let min = Vec3::checked(min.x, min.y, min.z)?;
        let max = Vec3::checked(max.x, max.y, max.z)?;
        if min.x > max.x || min.y > max.y || min.z > max.z {
            return Err(GeoError::InvalidTopology);
        }
        let in_box = |p: Vec3| {
            p.x >= min.x
                && p.x <= max.x
                && p.y >= min.y
                && p.y <= max.y
                && p.z >= min.z
                && p.z <= max.z
        };
        let mut selected = Vec::new();
        match domain {
            GroupDomain::Point => {
                for pt_idx in 0..self.len() {
                    if let Some(p) = self.point(pt_idx)
                        && in_box(p)
                    {
                        selected.push(pt_idx);
                    }
                }
            }
            GroupDomain::Prim => {
                let mut vtx_cursor = 0usize;
                let mut prim_idx = 0usize;
                for run in &*self.prim_runs {
                    let arity = run.arity as usize;
                    for _ in 0..run.prim_count {
                        let mut all_inside = arity > 0;
                        for k in 0..arity {
                            if let Some(pt) = self.vertex_point(vtx_cursor + k)
                                && let Some(p) = self.point(pt as usize)
                            {
                                if !in_box(p) {
                                    all_inside = false;
                                    break;
                                }
                            } else {
                                all_inside = false;
                                break;
                            }
                        }
                        if all_inside {
                            selected.push(prim_idx);
                        }
                        prim_idx += 1;
                        vtx_cursor += arity;
                    }
                }
            }
        }
        self.with_group(domain, name, &selected)
    }

    pub fn blast_group(&self, group_name: &str, negate: bool) -> Result<Self, GeoError> {
        let paused = AtomicBool::new(false);
        self.blast_group_cancellable(group_name, negate, usize::MAX, &paused)
    }

    /// Deletes elements selected by `group_name` (or non-selected elements when `negate` is `true`),
    /// compacting unused points and `vertex_points` while preserving valid primitive topology.
    /// Searches `GroupDomain::Prim` groups first, then `GroupDomain::Point` groups, and finally
    /// non-zero point attributes of the same name (allowing VEX wrangle masks to drive `Blast`).
    pub fn blast_group_cancellable(
        &self,
        group_name: &str,
        negate: bool,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        validate_attr_name(group_name)?;
        if let Some(prim_group) = self.group(GroupDomain::Prim, group_name) {
            let keep_prim: Vec<bool> = (0..self.prim_count())
                .map(|i| {
                    let in_grp = prim_group.contains(i);
                    let delete = if negate { !in_grp } else { in_grp };
                    !delete
                })
                .collect();
            return self.compact_by_kept_prims(&keep_prim, memory_limit, cancelled);
        }
        if let Some(pt_group) = self.group(GroupDomain::Point, group_name) {
            let keep_pt: Vec<bool> = (0..self.len())
                .map(|i| {
                    let in_grp = pt_group.contains(i);
                    let delete = if negate { !in_grp } else { in_grp };
                    !delete
                })
                .collect();
            return self.compact_by_kept_points(&keep_pt, memory_limit, cancelled);
        }
        if let Some(attr) = self.attribute(group_name) {
            let keep_pt: Vec<bool> = (0..self.len())
                .map(|i| {
                    let in_grp = match attr {
                        AttributeColumn::F32(c) => c.get(i).is_some_and(|v| v.get() != 0.0),
                        AttributeColumn::I32(c) => c.get(i).is_some_and(|v| v != 0),
                        AttributeColumn::U32(c) => c.get(i).is_some_and(|v| v != 0),
                        AttributeColumn::Bool(c) => c.get(i).unwrap_or(false),
                        AttributeColumn::Vec3(c) => c.get(i).is_some_and(|v| v.length() > 0.0),
                    };
                    let delete = if negate { !in_grp } else { in_grp };
                    !delete
                })
                .collect();
            return self.compact_by_kept_points(&keep_pt, memory_limit, cancelled);
        }
        Err(GeoError::AttributeNotFound)
    }

    fn compact_by_kept_prims(
        &self,
        keep_prim: &[bool],
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        let mut point_used = vec![false; self.len()];
        let mut vtx_cursor = 0usize;
        let mut prim_idx = 0usize;
        for run in &*self.prim_runs {
            let arity = run.arity as usize;
            for _ in 0..run.prim_count {
                if keep_prim.get(prim_idx).copied().unwrap_or(false) {
                    for k in 0..arity {
                        if let Some(pt) = self.vertex_point(vtx_cursor + k)
                            && let Some(slot) = point_used.get_mut(pt as usize)
                        {
                            *slot = true;
                        }
                    }
                }
                prim_idx += 1;
                vtx_cursor += arity;
            }
        }
        self.rebuild_compacted(&point_used, keep_prim, memory_limit, cancelled)
    }

    fn compact_by_kept_points(
        &self,
        keep_pt: &[bool],
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        let mut keep_prim = Vec::with_capacity(self.prim_count());
        let mut vtx_cursor = 0usize;
        for run in &*self.prim_runs {
            let arity = run.arity as usize;
            for _ in 0..run.prim_count {
                let mut all_kept = true;
                for k in 0..arity {
                    if let Some(pt) = self.vertex_point(vtx_cursor + k) {
                        if !keep_pt.get(pt as usize).copied().unwrap_or(false) {
                            all_kept = false;
                            break;
                        }
                    } else {
                        all_kept = false;
                        break;
                    }
                }
                keep_prim.push(all_kept);
                vtx_cursor += arity;
            }
        }
        self.rebuild_compacted(keep_pt, &keep_prim, memory_limit, cancelled)
    }

    fn rebuild_compacted(
        &self,
        keep_pt: &[bool],
        keep_prim: &[bool],
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        let mut old_to_new_pt = vec![u32::MAX; self.len()];
        let mut new_points = Vec::new();
        for (old_idx, &keep) in keep_pt.iter().enumerate() {
            if old_idx.is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            if keep && let Some(pos) = self.point(old_idx) {
                old_to_new_pt[old_idx] = new_points.len() as u32;
                new_points.push(pos);
            }
        }
        let mut new_runs: Vec<PrimRun> = Vec::new();
        let mut new_vertex_points: Vec<u32> = Vec::new();
        let mut vtx_cursor = 0usize;
        let mut prim_idx = 0usize;
        for run in &*self.prim_runs {
            let arity = run.arity as usize;
            for _ in 0..run.prim_count {
                if keep_prim.get(prim_idx).copied().unwrap_or(false) {
                    append_prim_run(&mut new_runs, run.kind, run.arity, 1)?;
                    for k in 0..arity {
                        let old_pt = self
                            .vertex_point(vtx_cursor + k)
                            .ok_or(GeoError::InvalidTopology)?
                            as usize;
                        let new_pt = *old_to_new_pt
                            .get(old_pt)
                            .filter(|&&v| v != u32::MAX)
                            .ok_or(GeoError::InvalidTopology)?;
                        new_vertex_points.push(new_pt);
                    }
                }
                prim_idx += 1;
                vtx_cursor += arity;
            }
        }

        let pt_bytes = new_points
            .len()
            .checked_mul(std::mem::size_of::<Vec3>())
            .ok_or(GeoError::LimitExceeded)?;
        let vtx_bytes = new_vertex_points
            .len()
            .checked_mul(std::mem::size_of::<u32>())
            .ok_or(GeoError::LimitExceeded)?;
        if pt_bytes
            .checked_add(vtx_bytes)
            .ok_or(GeoError::LimitExceeded)?
            > memory_limit
        {
            return Err(GeoError::LimitExceeded);
        }

        let mut out = GeoDetail::points(new_points);
        if !new_runs.is_empty() {
            out = out.with_primitives(new_runs, new_vertex_points)?;
        }
        for (name, col) in &*self.point_attributes {
            let filtered = filter_attribute_column(col, keep_pt)?;
            out = out.with_domain_attribute(AttrDomain::Point, name.clone(), filtered)?;
        }
        if !out.prim_runs.is_empty() {
            for (name, col) in &*self.prim_attributes {
                let filtered = filter_attribute_column(col, keep_prim)?;
                out = out.with_domain_attribute(AttrDomain::Prim, name.clone(), filtered)?;
            }
        }
        for (name, col) in &*self.detail_attributes {
            out = out.with_domain_attribute(AttrDomain::Detail, name.clone(), col.clone())?;
        }
        Ok(out)
    }

    pub fn uv_project(&self, mode: UvProjectMode, memory_limit: usize) -> Result<Self, GeoError> {
        let paused = AtomicBool::new(false);
        self.uv_project_cancellable(mode, memory_limit, &paused)
    }

    /// Computes normalized point UV coordinates (`v@uv` in $[0, 1]^2 \times \{0\}$) using
    /// planar, cylindrical, spherical, or box/triplanar projection over the geometry's bounds,
    /// preserving Copy-on-Write page sharing for `positions` and untouched attributes.
    pub fn uv_project_cancellable(
        &self,
        mode: UvProjectMode,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(GeoError::Cancelled);
        }
        let n = self.len();
        let bytes = n
            .checked_mul(std::mem::size_of::<Vec3>())
            .ok_or(GeoError::LimitExceeded)?;
        if bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }
        if n == 0 {
            return self.with_attribute("uv", AttributeColumn::vec3_values(Vec::new()));
        }
        let (lo, hi) = self.bounds().unwrap_or((Vec3::ZERO, Vec3::ZERO));
        let span_x = (hi.x - lo.x).max(1e-6);
        let span_y = (hi.y - lo.y).max(1e-6);
        let span_z = (hi.z - lo.z).max(1e-6);
        let center = Vec3::checked(
            0.5 * (lo.x + hi.x),
            0.5 * (lo.y + hi.y),
            0.5 * (lo.z + hi.z),
        )?;
        let tau = std::f32::consts::TAU;
        let pi = std::f32::consts::PI;

        let mut uvs = Vec::with_capacity(n);
        for i in 0..n {
            if i.is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            let p = self.point(i).unwrap_or(Vec3::ZERO);
            let nx = ((p.x - lo.x) / span_x).clamp(0.0, 1.0);
            let ny = ((p.y - lo.y) / span_y).clamp(0.0, 1.0);
            let nz = ((p.z - lo.z) / span_z).clamp(0.0, 1.0);
            let (u, v) = match mode {
                UvProjectMode::PlanarY => (nx, nz),
                UvProjectMode::PlanarZ => (nx, ny),
                UvProjectMode::PlanarX => (nz, ny),
                UvProjectMode::Cylindrical => {
                    let dx = p.x - center.x;
                    let dz = p.z - center.z;
                    let angle = dz.atan2(dx);
                    let u_ang = ((angle + pi) / tau).clamp(0.0, 1.0);
                    (u_ang, ny)
                }
                UvProjectMode::Spherical => {
                    let rel = p.sub_checked(center)?.normalized_or_up();
                    let u_ang = ((rel.z.atan2(rel.x) + pi) / tau).clamp(0.0, 1.0);
                    let v_lat = (0.5 + rel.y.clamp(-1.0, 1.0).asin() / pi).clamp(0.0, 1.0);
                    (u_ang, v_lat)
                }
                UvProjectMode::Box => {
                    let dx = ((p.x - center.x) / span_x).abs();
                    let dy = ((p.y - center.y) / span_y).abs();
                    let dz = ((p.z - center.z) / span_z).abs();
                    if dy >= dx && dy >= dz {
                        (nx, nz)
                    } else if dx >= dy && dx >= dz {
                        (nz, ny)
                    } else {
                        (nx, ny)
                    }
                }
            };
            uvs.push(Vec3::checked(u, v, 0.0)?);
        }
        self.with_attribute("uv", AttributeColumn::vec3_values(uvs))
    }

    pub fn translated_cancellable(
        &self,
        offset: Vec3,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        let bytes = self
            .len()
            .checked_mul(std::mem::size_of::<Vec3>())
            .ok_or(GeoError::LimitExceeded)?;
        if bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }
        let mut pages = Vec::with_capacity(self.positions.pages.len());
        for page in &*self.positions.pages {
            if cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            let values = page
                .values
                .iter()
                .copied()
                .map(|p| p.add_checked(offset).map_err(GeoError::from))
                .collect::<Result<Vec<_>, _>>()?;
            pages.push(Page::from_vec(values));
        }
        let mut next = self.clone();
        next.positions = PagedColumn {
            pages: Arc::from(pages),
            len: self.len(),
        };
        Ok(next)
    }

    /// Replaces every point position while sharing topology, attributes, and groups.
    pub fn with_positions(&self, positions: Vec<Vec3>) -> Result<Self, GeoError> {
        if positions.len() != self.len() {
            return Err(GeoError::AttributeLengthMismatch {
                expected: self.len(),
                actual: positions.len(),
            });
        }
        let mut next = self.clone();
        next.positions = PagedColumn::from_vec(positions);
        Ok(next)
    }

    /// Reverses the vertex order of every polygon, flipping surface orientation, and inverts every
    /// tetrahedron. Vertex attributes travel with their vertices; point and primitive data are
    /// shared. Polylines are unchanged.
    pub fn with_reversed_winding(&self) -> Result<Self, GeoError> {
        let mut order = Vec::with_capacity(self.vertex_count());
        let mut cursor = 0usize;
        for run in &*self.prim_runs {
            let arity = run.arity as usize;
            for _ in 0..run.prim_count {
                match run.kind {
                    PrimKind::Polygon => order.extend((cursor..cursor + arity).rev()),
                    PrimKind::Polyline | PrimKind::Packed | PrimKind::Volume => {
                        order.extend(cursor..cursor + arity);
                    }
                    // One swap inverts an element; reversing all four corners would not.
                    PrimKind::Tetrahedron => {
                        order.extend([cursor, cursor + 2, cursor + 1, cursor + 3]);
                    }
                }
                cursor += arity;
            }
        }
        let mut vertex_points = Vec::with_capacity(order.len());
        for &vertex in &order {
            vertex_points.push(
                self.vertex_point(vertex)
                    .ok_or(GeoError::IndexOutOfBounds)?,
            );
        }
        let mut vertex_attributes = BTreeMap::new();
        for (name, column) in &*self.vertex_attributes {
            vertex_attributes.insert(name.clone(), column.selected(&order)?);
        }
        let mut next = self.clone();
        next.vertex_points = PagedColumn::from_vec(vertex_points);
        next.vertex_attributes = Arc::new(vertex_attributes);
        Ok(next)
    }

    /// Produces a new detail while copying only the position page containing `index`.
    /// All non-position attribute columns, topology, and groups remain shared via `Arc`.
    pub fn with_point(&self, index: usize, point: Vec3) -> Result<Self, GeoError> {
        let mut next = self.clone();
        next.positions = self.positions.with_element(index, point)?;
        Ok(next)
    }

    #[must_use]
    pub fn hash(&self) -> blake3::Hash {
        let mut h = blake3::Hasher::new();
        h.update(&(self.len() as u64).to_le_bytes());
        for page in &*self.positions.pages {
            for p in &*page.values {
                for v in [p.x, p.y, p.z] {
                    h.update(&v.to_bits().to_le_bytes());
                }
            }
        }
        if !self.point_attributes.is_empty() {
            h.update(&(self.point_attributes.len() as u64).to_le_bytes());
            for (name, column) in &*self.point_attributes {
                h.update(&(name.len() as u64).to_le_bytes());
                h.update(name.as_bytes());
                h.update(column.hash().as_bytes());
            }
        }
        if !self.prim_runs.is_empty() {
            h.update(b"topology-v1");
            h.update(&(self.prim_runs.len() as u64).to_le_bytes());
            for run in &*self.prim_runs {
                let kind_tag: u8 = match run.kind {
                    PrimKind::Polygon => 1,
                    PrimKind::Polyline => 2,
                    PrimKind::Packed => 3,
                    PrimKind::Tetrahedron => 4,
                    PrimKind::Volume => 5,
                };
                h.update(&[kind_tag]);
                h.update(&run.arity.to_le_bytes());
                h.update(&(run.prim_count as u64).to_le_bytes());
            }
            h.update(self.vertex_points.hash().as_bytes());
        }
        for (tag, map) in [
            (b"vertex-attrs" as &[u8], &self.vertex_attributes),
            (b"prim-attrs", &self.prim_attributes),
            (b"detail-attrs", &self.detail_attributes),
        ] {
            if !map.is_empty() {
                h.update(tag);
                h.update(&(map.len() as u64).to_le_bytes());
                for (name, column) in &**map {
                    h.update(&(name.len() as u64).to_le_bytes());
                    h.update(name.as_bytes());
                    h.update(column.hash().as_bytes());
                }
            }
        }
        for (tag, groups) in [
            (b"point-groups" as &[u8], &self.point_groups),
            (b"prim-groups", &self.prim_groups),
        ] {
            if !groups.is_empty() {
                h.update(tag);
                h.update(&(groups.len() as u64).to_le_bytes());
                for (name, group) in &**groups {
                    h.update(&(name.len() as u64).to_le_bytes());
                    h.update(name.as_bytes());
                    h.update(group.hash().as_bytes());
                }
            }
        }
        if let Some(entries) = &self.volumes {
            h.update(b"volumes-v1");
            h.update(&(entries.len() as u64).to_le_bytes());
            for entry in &**entries {
                h.update(entry.grid.hash().as_bytes());
            }
        }
        if let Some(table) = &self.packed {
            table.update_hash(&mut h);
        }
        h.finalize()
    }

    #[must_use]
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let first = self.point(0)?;
        let mut lo = first;
        let mut hi = first;
        for page in &*self.positions.pages {
            for p in &*page.values {
                lo.x = lo.x.min(p.x);
                lo.y = lo.y.min(p.y);
                lo.z = lo.z.min(p.z);
                hi.x = hi.x.max(p.x);
                hi.y = hi.y.max(p.y);
                hi.z = hi.z.max(p.z);
            }
        }
        // Instances reach past their points, by as much as their prototypes extend.
        // Volumes likewise reach past theirs, by as far as their voxels go.
        for (low, high) in [self.packed_bounds(), self.volume_bounds()]
            .into_iter()
            .flatten()
        {
            lo = Vec3 {
                x: lo.x.min(low.x),
                y: lo.y.min(low.y),
                z: lo.z.min(low.z),
            };
            hi = Vec3 {
                x: hi.x.max(high.x),
                y: hi.y.max(high.y),
                z: hi.z.max(high.z),
            };
        }
        Some((lo, hi))
    }

    #[must_use]
    pub fn page_shared_with(&self, other: &Self, page: usize) -> bool {
        self.positions.page_shared_with(&other.positions, page)
    }

    #[must_use]
    pub fn attribute_page_shared_with(&self, other: &Self, name: &str, page: usize) -> bool {
        self.attribute(name).is_some_and(|a| {
            other
                .attribute(name)
                .is_some_and(|b| a.page_shared_with(b, page))
        })
    }
}

fn append_prim_run(
    runs: &mut Vec<PrimRun>,
    kind: PrimKind,
    arity: u32,
    count: usize,
) -> Result<(), GeoError> {
    if count == 0 {
        return Ok(());
    }
    if let Some(last) = runs.last_mut()
        && last.kind == kind
        && last.arity == arity
    {
        last.prim_count = last
            .prim_count
            .checked_add(count)
            .ok_or(GeoError::LimitExceeded)?;
    } else {
        runs.push(PrimRun {
            kind,
            arity,
            prim_count: count,
        });
    }
    Ok(())
}

fn reduce_f32(values: &[f32], mode: PromoteMode) -> Result<f32, GeoError> {
    let Some(&first) = values.first() else {
        return Ok(0.0);
    };
    match mode {
        PromoteMode::Sum | PromoteMode::Average => {
            let mut sum = 0.0f32;
            for &v in values {
                sum += v;
                if !sum.is_finite() {
                    return Err(GeoError::NonFinite);
                }
            }
            if mode == PromoteMode::Average {
                let avg = sum / (values.len() as f32);
                Ok(FiniteF32::checked(avg)?.get())
            } else {
                Ok(FiniteF32::checked(sum)?.get())
            }
        }
        PromoteMode::Min => {
            let mut acc = first;
            for &v in &values[1..] {
                acc = acc.min(v);
            }
            Ok(FiniteF32::checked(acc)?.get())
        }
        PromoteMode::Max => {
            let mut acc = first;
            for &v in &values[1..] {
                acc = acc.max(v);
            }
            Ok(FiniteF32::checked(acc)?.get())
        }
    }
}

fn reduce_vec3(values: &[Vec3], mode: PromoteMode) -> Result<Vec3, GeoError> {
    let Some(&first) = values.first() else {
        return Ok(Vec3::ZERO);
    };
    match mode {
        PromoteMode::Sum | PromoteMode::Average => {
            let mut sum = Vec3::ZERO;
            for &v in values {
                sum = sum.add_checked(v)?;
            }
            if mode == PromoteMode::Average {
                Ok(sum.scale_checked(1.0 / (values.len() as f32))?)
            } else {
                Ok(sum)
            }
        }
        PromoteMode::Min => {
            let mut acc = first;
            for &v in &values[1..] {
                acc = Vec3::checked(acc.x.min(v.x), acc.y.min(v.y), acc.z.min(v.z))?;
            }
            Ok(acc)
        }
        PromoteMode::Max => {
            let mut acc = first;
            for &v in &values[1..] {
                acc = Vec3::checked(acc.x.max(v.x), acc.y.max(v.y), acc.z.max(v.z))?;
            }
            Ok(acc)
        }
    }
}

fn filter_attribute_column(
    col: &AttributeColumn,
    keep: &[bool],
) -> Result<AttributeColumn, GeoError> {
    match col {
        AttributeColumn::F32(c) => {
            let mut vals = Vec::new();
            for (idx, &k) in keep.iter().enumerate() {
                if k && let Some(v) = c.get(idx) {
                    vals.push(v.get());
                }
            }
            AttributeColumn::f32_checked(vals)
        }
        AttributeColumn::I32(c) => {
            let mut vals = Vec::new();
            for (idx, &k) in keep.iter().enumerate() {
                if k && let Some(v) = c.get(idx) {
                    vals.push(v);
                }
            }
            Ok(AttributeColumn::i32_values(vals))
        }
        AttributeColumn::U32(c) => {
            let mut vals = Vec::new();
            for (idx, &k) in keep.iter().enumerate() {
                if k && let Some(v) = c.get(idx) {
                    vals.push(v);
                }
            }
            Ok(AttributeColumn::u32_values(vals))
        }
        AttributeColumn::Bool(c) => {
            let mut vals = Vec::new();
            for (idx, &k) in keep.iter().enumerate() {
                if k && let Some(v) = c.get(idx) {
                    vals.push(v);
                }
            }
            Ok(AttributeColumn::bool_values(vals))
        }
        AttributeColumn::Vec3(c) => {
            let mut vals = Vec::new();
            for (idx, &k) in keep.iter().enumerate() {
                if k && let Some(v) = c.get(idx) {
                    vals.push(v);
                }
            }
            Ok(AttributeColumn::vec3_values(vals))
        }
    }
}

pub const NO_HALF_EDGE: u32 = u32::MAX;

/// Lazy, immutable CSR half-edge topological index over `GeoDetail` primitives.
/// Each vertex index `0..vertex_count` acts as an implicit half-edge ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopologyIndex {
    topology_hash: blake3::Hash,
    prim_vertex_start: Vec<u32>,
    prim_arity: Vec<u32>,
    prim_kinds: Vec<PrimKind>,
    vertex_prim: Vec<u32>,
    next_he: Vec<u32>,
    prev_he: Vec<u32>,
    twin_he: Vec<u32>,
    boundary_edge_count: usize,
    non_manifold_edge_count: usize,
}

impl TopologyIndex {
    #[must_use]
    pub fn build(geo: &GeoDetail) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(b"topology-index-v1");
        h.update(&(geo.prim_runs.len() as u64).to_le_bytes());
        for run in &*geo.prim_runs {
            let kind_tag: u8 = match run.kind {
                PrimKind::Polygon => 1,
                PrimKind::Polyline => 2,
                PrimKind::Packed => 3,
                PrimKind::Tetrahedron => 4,
                PrimKind::Volume => 5,
            };
            h.update(&[kind_tag]);
            h.update(&run.arity.to_le_bytes());
            h.update(&(run.prim_count as u64).to_le_bytes());
        }
        h.update(geo.vertex_points.hash().as_bytes());
        let topology_hash = h.finalize();

        let prim_count = geo.prim_count();
        let vtx_count = geo.vertex_count();
        let mut prim_vertex_start = Vec::with_capacity(prim_count);
        let mut prim_arity = Vec::with_capacity(prim_count);
        let mut prim_kinds = Vec::with_capacity(prim_count);
        let mut vertex_prim = vec![0u32; vtx_count];
        let mut next_he = vec![NO_HALF_EDGE; vtx_count];
        let mut prev_he = vec![NO_HALF_EDGE; vtx_count];
        let mut twin_he = vec![NO_HALF_EDGE; vtx_count];

        // (min_pt, max_pt, forward_dir, he)
        let mut directed_edges: Vec<(u32, u32, bool, u32)> = Vec::with_capacity(vtx_count);
        let mut non_manifold_edge_count = 0usize;

        let mut vtx_cursor = 0usize;
        for run in &*geo.prim_runs {
            let arity = run.arity as usize;
            for _ in 0..run.prim_count {
                let prim_idx = prim_vertex_start.len() as u32;
                prim_vertex_start.push(vtx_cursor as u32);
                prim_arity.push(run.arity);
                prim_kinds.push(run.kind);
                for k in 0..arity {
                    let he = vtx_cursor + k;
                    if let Some(slot) = vertex_prim.get_mut(he) {
                        *slot = prim_idx;
                    }
                    match run.kind {
                        PrimKind::Polygon => {
                            let nxt = vtx_cursor + ((k + 1) % arity);
                            let prv = vtx_cursor + ((k + arity - 1) % arity);
                            if let Some(slot) = next_he.get_mut(he) {
                                *slot = nxt as u32;
                            }
                            if let Some(slot) = prev_he.get_mut(he) {
                                *slot = prv as u32;
                            }
                            if let (Some(u), Some(v)) =
                                (geo.vertex_point(he), geo.vertex_point(nxt))
                            {
                                if u == v {
                                    non_manifold_edge_count += 1;
                                } else if u < v {
                                    directed_edges.push((u, v, true, he as u32));
                                } else {
                                    directed_edges.push((v, u, false, he as u32));
                                }
                            }
                        }
                        // A packed primitive has one vertex, and so no edges at all. A tetrahedron's
                        // edges are not surface edges, so they must not affect manifold checks.
                        PrimKind::Polyline
                        | PrimKind::Packed
                        | PrimKind::Tetrahedron
                        | PrimKind::Volume => {
                            if k + 1 < arity
                                && let Some(slot) = next_he.get_mut(he)
                            {
                                *slot = (he + 1) as u32;
                            }
                            if k > 0
                                && let Some(slot) = prev_he.get_mut(he)
                            {
                                *slot = (he - 1) as u32;
                            }
                        }
                    }
                }
                vtx_cursor += arity;
            }
        }

        directed_edges.sort_unstable_by_key(|&(lo, hi, _, he)| (lo, hi, he));
        let mut boundary_edge_count = 0usize;
        let mut idx = 0usize;
        while idx < directed_edges.len() {
            let (lo, hi, _, _) = directed_edges[idx];
            let mut end = idx + 1;
            while end < directed_edges.len()
                && directed_edges[end].0 == lo
                && directed_edges[end].1 == hi
            {
                end += 1;
            }
            let group = &directed_edges[idx..end];
            if group.len() == 1 {
                boundary_edge_count += 1;
            } else if group.len() == 2 {
                let a = group[0];
                let b = group[1];
                if a.2 != b.2 {
                    if let Some(slot) = twin_he.get_mut(a.3 as usize) {
                        *slot = b.3;
                    }
                    if let Some(slot) = twin_he.get_mut(b.3 as usize) {
                        *slot = a.3;
                    }
                } else {
                    non_manifold_edge_count += 1;
                }
            } else {
                non_manifold_edge_count += 1;
            }
            idx = end;
        }

        Self {
            topology_hash,
            prim_vertex_start,
            prim_arity,
            prim_kinds,
            vertex_prim,
            next_he,
            prev_he,
            twin_he,
            boundary_edge_count,
            non_manifold_edge_count,
        }
    }

    #[must_use]
    pub const fn topology_hash(&self) -> blake3::Hash {
        self.topology_hash
    }

    #[must_use]
    pub const fn boundary_edge_count(&self) -> usize {
        self.boundary_edge_count
    }

    #[must_use]
    pub const fn non_manifold_edge_count(&self) -> usize {
        self.non_manifold_edge_count
    }

    #[must_use]
    pub const fn is_manifold(&self) -> bool {
        self.non_manifold_edge_count == 0
    }

    #[must_use]
    pub const fn is_closed_manifold(&self) -> bool {
        self.non_manifold_edge_count == 0
            && self.boundary_edge_count == 0
            && !self.prim_arity.is_empty()
    }

    #[must_use]
    pub fn twin(&self, he: u32) -> Option<u32> {
        self.twin_he
            .get(he as usize)
            .copied()
            .filter(|&v| v != NO_HALF_EDGE)
    }

    #[must_use]
    pub fn next(&self, he: u32) -> Option<u32> {
        self.next_he
            .get(he as usize)
            .copied()
            .filter(|&v| v != NO_HALF_EDGE)
    }

    #[must_use]
    pub fn prev(&self, he: u32) -> Option<u32> {
        self.prev_he
            .get(he as usize)
            .copied()
            .filter(|&v| v != NO_HALF_EDGE)
    }

    #[must_use]
    pub fn prim_of_vertex(&self, he: u32) -> Option<u32> {
        self.vertex_prim.get(he as usize).copied()
    }

    #[must_use]
    pub fn prim_vertex_start(&self, prim: u32) -> Option<u32> {
        self.prim_vertex_start.get(prim as usize).copied()
    }

    #[must_use]
    pub fn prim_arity(&self, prim: u32) -> Option<u32> {
        self.prim_arity.get(prim as usize).copied()
    }

    #[must_use]
    pub fn prim_kind(&self, prim: u32) -> Option<PrimKind> {
        self.prim_kinds.get(prim as usize).copied()
    }

    /// Counts points whose incident polygon corners form more than one edge-connected fan. Such a
    /// point pinches the surface even though every edge may be manifold, which
    /// [`Self::is_closed_manifold`] does not detect.
    #[must_use]
    pub fn non_manifold_vertex_count(&self, geo: &GeoDetail) -> usize {
        let corners = self.next_he.len();
        let mut visited = vec![false; corners];
        let mut fans = vec![0u32; geo.len()];
        for start in 0..corners as u32 {
            if visited[start as usize]
                || self.prim_of_vertex(start).and_then(|p| self.prim_kind(p))
                    != Some(PrimKind::Polygon)
            {
                continue;
            }
            let Some(point) = geo.vertex_point(start as usize) else {
                continue;
            };
            fans[point as usize] += 1;
            visited[start as usize] = true;
            // Sweep around the point in both directions until the fan closes or hits a boundary.
            let mut corner = start;
            while let Some(step) = self.prev(corner).and_then(|prev| self.twin(prev)) {
                if visited[step as usize] {
                    break;
                }
                visited[step as usize] = true;
                corner = step;
            }
            corner = start;
            while let Some(step) = self.twin(corner).and_then(|twin| self.next(twin)) {
                if visited[step as usize] {
                    break;
                }
                visited[step as usize] = true;
                corner = step;
            }
        }
        fans.iter().filter(|&&count| count > 1).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_hash_and_bounds() {
        let g = GeoDetail::points(vec![
            Vec3::ZERO,
            Vec3::checked(2., 3., 4.).unwrap_or(Vec3::ZERO),
        ]);
        assert_eq!(g.hash(), g.clone().hash());
        assert_eq!(
            g.bounds().map(|x| x.1),
            Some(Vec3::checked(2., 3., 4.).unwrap_or(Vec3::ZERO))
        );
    }
    #[test]
    fn resource_limit_prevents_work() {
        let g = GeoDetail::points(vec![Vec3::ZERO]);
        assert!(matches!(
            g.translated(Vec3::ZERO, 1),
            Err(GeoError::LimitExceeded)
        ));
    }
    #[test]
    fn edit_copies_only_affected_page() {
        let g = GeoDetail::points(vec![Vec3::ZERO; PAGE_LEN + 1]);
        let edited = g.with_point(0, Vec3::checked(1., 0., 0.).unwrap_or(Vec3::ZERO));
        assert!(
            matches!(edited, Ok(ref value) if !g.page_shared_with(value, 0) && g.page_shared_with(value, 1))
        );
    }
    #[test]
    fn typed_attribute_columns_validate_and_preserve_cow_page_sharing() {
        let count = PAGE_LEN + 2;
        let base = GeoDetail::points(vec![Vec3::ZERO; count]);

        // Reject NaN/Inf in f32 attribute column at ingress
        assert_eq!(
            AttributeColumn::f32_checked(vec![0.0, f32::NAN]).err(),
            Some(GeoError::NonFinite)
        );
        assert_eq!(
            AttributeColumn::f32_checked(vec![f32::INFINITY]).err(),
            Some(GeoError::NonFinite)
        );

        // Reject reserved "P" or invalid identifier names and length mismatches
        let valid_short = AttributeColumn::i32_values(vec![1; 4]);
        assert_eq!(
            base.with_attribute("P", valid_short.clone()).err(),
            Some(GeoError::ReservedAttributeName)
        );
        assert_eq!(
            base.with_attribute("1bad", valid_short.clone()).err(),
            Some(GeoError::InvalidAttributeName)
        );
        assert_eq!(
            base.with_attribute("id", valid_short).err(),
            Some(GeoError::AttributeLengthMismatch {
                expected: count,
                actual: 4
            })
        );

        // Attach pscale (f32) and id (i32) attributes
        let pscale_col = AttributeColumn::f32_checked(vec![1.0; count]);
        assert!(pscale_col.is_ok());
        let Ok(pscale_col) = pscale_col else {
            return;
        };
        let with_pscale = base.with_attribute("pscale", pscale_col);
        assert!(with_pscale.is_ok());
        let Ok(with_pscale) = with_pscale else {
            return;
        };

        // Adding an attribute shares all position pages with `base`
        assert!(base.page_shared_with(&with_pscale, 0));
        assert!(base.page_shared_with(&with_pscale, 1));
        assert_ne!(base.hash(), with_pscale.hash());

        // Editing element 0 of `pscale` copies only page 0 of `pscale`;
        // page 1 of `pscale` and all pages of `positions` remain shared!
        let edited_pscale = with_pscale.with_f32_attribute_element("pscale", 0, 2.5);
        assert!(edited_pscale.is_ok());
        let Ok(edited_pscale) = edited_pscale else {
            return;
        };
        assert!(!with_pscale.attribute_page_shared_with(&edited_pscale, "pscale", 0));
        assert!(with_pscale.attribute_page_shared_with(&edited_pscale, "pscale", 1));
        assert!(with_pscale.page_shared_with(&edited_pscale, 0));
        assert!(with_pscale.page_shared_with(&edited_pscale, 1));
        assert_ne!(with_pscale.hash(), edited_pscale.hash());

        // Translating positions copies position pages but shares all non-position attribute pages
        let translated =
            edited_pscale.translated(Vec3::checked(1.0, 2.0, 3.0).unwrap_or(Vec3::ZERO), 1 << 24);
        assert!(translated.is_ok());
        let Ok(translated) = translated else {
            return;
        };
        assert!(edited_pscale.attribute_page_shared_with(&translated, "pscale", 0));
        assert!(edited_pscale.attribute_page_shared_with(&translated, "pscale", 1));
    }

    #[test]
    fn topology_multi_domain_attributes_and_groups_validate_and_hash() {
        let base = GeoDetail::points(vec![
            Vec3::ZERO,
            Vec3::checked(1.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(1.0, 1.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(0.0, 1.0, 0.0).unwrap_or(Vec3::ZERO),
        ]);

        // Polygon requires arity >= 3
        assert_eq!(
            base.with_primitives(
                vec![PrimRun {
                    kind: PrimKind::Polygon,
                    arity: 2,
                    prim_count: 1,
                }],
                vec![0, 1],
            )
            .err(),
            Some(GeoError::InvalidTopology)
        );

        // Out-of-bounds vertex point index rejected
        assert_eq!(
            base.with_primitives(
                vec![PrimRun {
                    kind: PrimKind::Polygon,
                    arity: 3,
                    prim_count: 1,
                }],
                vec![0, 1, 99],
            )
            .err(),
            Some(GeoError::IndexOutOfBounds)
        );

        // Valid 2-triangle mesh (6 vertices, 2 prims)
        let mesh = base.with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 2,
            }],
            vec![0, 1, 2, 0, 2, 3],
        );
        assert!(mesh.is_ok());
        let Ok(mesh) = mesh else {
            return;
        };
        assert_eq!(mesh.prim_count(), 2);
        assert_eq!(mesh.vertex_count(), 6);
        assert!(base.page_shared_with(&mesh, 0));
        assert_ne!(base.hash(), mesh.hash());

        // Vertex attribute (length 6), Prim attribute (length 2), Detail attribute (length 1)
        let with_vtx = mesh.with_domain_attribute(
            AttrDomain::Vertex,
            "uv_u",
            AttributeColumn::u32_values(vec![0, 1, 1, 0, 1, 0]),
        );
        assert!(with_vtx.is_ok());
        let Ok(with_vtx) = with_vtx else {
            return;
        };
        let with_prim = with_vtx.with_domain_attribute(
            AttrDomain::Prim,
            "mat_id",
            AttributeColumn::i32_values(vec![10, 20]),
        );
        assert!(with_prim.is_ok());
        let Ok(with_prim) = with_prim else {
            return;
        };
        let with_detail = with_prim.with_domain_attribute(
            AttrDomain::Detail,
            "solver_substeps",
            AttributeColumn::u32_values(vec![8]),
        );
        assert!(with_detail.is_ok());
        let Ok(with_detail) = with_detail else {
            return;
        };

        // Point group and Prim group
        let with_pt_grp = with_detail.with_group(GroupDomain::Point, "pinned", &[0, 3]);
        assert!(with_pt_grp.is_ok());
        let Ok(with_pt_grp) = with_pt_grp else {
            return;
        };
        let with_prim_grp = with_pt_grp.with_group(GroupDomain::Prim, "top_half", &[1]);
        assert!(with_prim_grp.is_ok());
        let Ok(final_geo) = with_prim_grp else {
            return;
        };

        assert_eq!(
            final_geo
                .group(GroupDomain::Point, "pinned")
                .map(ElementGroup::selected_count),
            Some(2)
        );
        assert_eq!(
            final_geo.group(GroupDomain::Prim, "top_half").map(|g| (
                g.selected_count(),
                g.contains(0),
                g.contains(1)
            )),
            Some((1, false, true))
        );
        assert_ne!(with_detail.hash(), final_geo.hash());
    }

    #[test]
    fn topology_index_normals_area_and_enclosed_volume() {
        // Build a closed manifold 2x3x4 box (8 points, 6 quads, 24 vertices)
        let pts = vec![
            Vec3::checked(0.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 3.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(0.0, 3.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(0.0, 0.0, 4.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 0.0, 4.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 3.0, 4.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(0.0, 3.0, 4.0).unwrap_or(Vec3::ZERO),
        ];
        let vtx = vec![
            0, 3, 2, 1, // -Z
            4, 5, 6, 7, // +Z
            0, 1, 5, 4, // -Y
            3, 7, 6, 2, // +Y
            0, 4, 7, 3, // -X
            1, 2, 6, 5, // +X
        ];
        let box_mesh = GeoDetail::points(pts).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 4,
                prim_count: 6,
            }],
            vtx,
        );
        assert!(box_mesh.is_ok());
        let Ok(box_mesh) = box_mesh else {
            return;
        };

        let topo = box_mesh.topology_index();
        assert!(topo.is_manifold());
        assert!(topo.is_closed_manifold());
        assert_eq!(topo.boundary_edge_count(), 0);
        assert_eq!(topo.non_manifold_edge_count(), 0);
        // Every half-edge has a valid twin and twin(twin(he)) == he
        for he in 0..24u32 {
            let tw = topo.twin(he);
            assert!(tw.is_some());
            if let Some(t) = tw {
                assert_eq!(topo.twin(t), Some(he));
            }
        }

        // Surface area of 2x3x4 box = 2*(6 + 8 + 12) = 52.0; Volume = 2*3*4 = 24.0
        assert!((box_mesh.surface_area() - 52.0).abs() < 1e-4);
        assert!((box_mesh.enclosed_volume() - 24.0).abs() < 1e-4);

        // Compute normals and verify `N` attribute is attached and unit-length
        let with_normals = box_mesh.compute_normals(1 << 20);
        assert!(with_normals.is_ok());
        if let Ok(with_normals) = with_normals {
            let n_col = with_normals
                .attribute("N")
                .and_then(AttributeColumn::as_vec3);
            assert!(n_col.is_some());
            if let Some(n_col) = n_col {
                assert_eq!(n_col.len(), 8);
                for i in 0..8 {
                    if let Some(n) = n_col.get(i) {
                        assert!((n.length() - 1.0).abs() < 1e-4);
                    }
                }
            }
        }
    }

    #[test]
    fn catmull_clark_subdivide_preserves_closed_manifold_and_quadruples_faces() {
        let pts = vec![
            Vec3::checked(0.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 2.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(0.0, 2.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(0.0, 0.0, 2.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 0.0, 2.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 2.0, 2.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(0.0, 2.0, 2.0).unwrap_or(Vec3::ZERO),
        ];
        let vtx = vec![
            0, 3, 2, 1, // -Z
            4, 5, 6, 7, // +Z
            0, 1, 5, 4, // -Y
            3, 7, 6, 2, // +Y
            0, 4, 7, 3, // -X
            1, 2, 6, 5, // +X
        ];
        let Ok(box_mesh) = GeoDetail::points(pts).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 4,
                prim_count: 6,
            }],
            vtx,
        ) else {
            return;
        };

        let sub1 = box_mesh.subdivide(1, 1 << 20);
        assert!(sub1.is_ok());
        if let Ok(sub1) = sub1 {
            // 8 orig + 6 face + 12 edge = 26 points; 6 * 4 = 24 quads; 96 vertices
            assert_eq!(sub1.len(), 26);
            assert_eq!(sub1.prim_count(), 24);
            assert_eq!(sub1.vertex_count(), 96);
            assert!(sub1.topology_index().is_closed_manifold());
            assert!(sub1.enclosed_volume() > 2.5 && sub1.enclosed_volume() < 8.0);
        }

        let sub2 = box_mesh.subdivide(2, 1 << 20);
        assert!(sub2.is_ok());
        if let Ok(sub2) = sub2 {
            // 26 + 24 + 48 = 98 points; 24 * 4 = 96 quads; 384 vertices
            assert_eq!(sub2.len(), 98);
            assert_eq!(sub2.prim_count(), 96);
            assert_eq!(sub2.vertex_count(), 384);
            assert!(sub2.topology_index().is_closed_manifold());
            assert!(sub2.enclosed_volume() > 2.0 && sub2.enclosed_volume() < 8.0);
        }
    }

    #[test]
    fn poly_extrude_group_and_box_preserve_manifold_and_increase_volume() {
        let pts = vec![
            Vec3::checked(0.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 2.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(0.0, 2.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(0.0, 0.0, 2.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 0.0, 2.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 2.0, 2.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(0.0, 2.0, 2.0).unwrap_or(Vec3::ZERO),
        ];
        let vtx = vec![
            0, 3, 2, 1, // -Z
            4, 5, 6, 7, // +Z
            0, 1, 5, 4, // -Y
            3, 7, 6, 2, // +Y (top face, prim 3)
            0, 4, 7, 3, // -X
            1, 2, 6, 5, // +X
        ];
        let Ok(box_mesh) = GeoDetail::points(pts).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 4,
                prim_count: 6,
            }],
            vtx,
        ) else {
            return;
        };

        // Group the +Y top face by bounds [0, 1.9, 0]..[2, 2.1, 2] and extrude only that face by 1.0
        let paused = AtomicBool::new(false);
        let Ok(with_top_grp) = box_mesh.group_by_bounds(
            GroupDomain::Prim,
            "top_face",
            Vec3::checked(-0.1, 1.9, -0.1).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.1, 2.1, 2.1).unwrap_or(Vec3::ZERO),
        ) else {
            return;
        };
        assert_eq!(
            with_top_grp
                .group(GroupDomain::Prim, "top_face")
                .map(ElementGroup::selected_count),
            Some(1)
        );
        let ext_top = with_top_grp.poly_extrude_group_cancellable(
            Some("top_face"),
            1.0,
            0.0,
            1 << 20,
            &paused,
        );
        assert!(ext_top.is_ok());
        if let Ok(ext_top) = ext_top {
            // 5 unselected quads + 1 cap quad + 4 side walls = 10 quads, 8 + 4 = 12 points
            assert_eq!(ext_top.len(), 12);
            assert_eq!(ext_top.prim_count(), 10);
            assert_eq!(ext_top.vertex_count(), 40);
            assert!(ext_top.topology_index().is_closed_manifold());
            // Original 2x2x2 volume = 8.0; extruding 2x2 face by 1.0 yields 8.0 + 4.0 = 12.0
            assert!((ext_top.enclosed_volume() - 12.0).abs() < 1e-4);
            assert!((ext_top.surface_area() - 32.0).abs() < 1e-4);
        }

        // Extrude all 6 faces of the box with distance 0.5 and inset 0.2
        let ext_all = box_mesh.poly_extrude(0.5, 0.2, 1 << 20);
        assert!(ext_all.is_ok());
        if let Ok(ext_all) = ext_all {
            assert_eq!(ext_all.len(), 32);
            assert_eq!(ext_all.prim_count(), 30);
            assert_eq!(ext_all.vertex_count(), 120);
            assert!(ext_all.topology_index().is_closed_manifold());
            assert!(ext_all.enclosed_volume() > box_mesh.enclosed_volume());
            assert!(ext_all.surface_area() > box_mesh.surface_area());
        }
    }

    #[test]
    fn promote_attribute_and_blast_group_preserve_topology_and_cow() {
        // Two-quad strip: 6 points, 2 quads (0,1,2,3) and (1,4,5,2)
        let pts = vec![
            Vec3::checked(0.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(1.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(1.0, 0.0, 1.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(0.0, 0.0, 1.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.0, 0.0, 1.0).unwrap_or(Vec3::ZERO),
        ];
        let Ok(mesh) = GeoDetail::points(pts).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 4,
                prim_count: 2,
            }],
            vec![0, 1, 2, 3, 1, 4, 5, 2],
        ) else {
            return;
        };
        let Ok(heat_col) = AttributeColumn::f32_checked(vec![1.0, 2.0, 3.0, 4.0, 10.0, 12.0])
        else {
            return;
        };
        let Ok(with_heat) = mesh.with_attribute("heat", heat_col) else {
            return;
        };

        // Promote Point -> Prim with Average: quad 0 has (1+2+3+4)/4 = 2.5; quad 1 has (2+10+12+3)/4 = 6.75
        let Ok(prim_avg) = with_heat.promote_attribute(
            "heat",
            AttrDomain::Point,
            AttrDomain::Prim,
            PromoteMode::Average,
        ) else {
            return;
        };
        assert!(with_heat.page_shared_with(&prim_avg, 0));
        assert!(
            prim_avg
                .domain_attribute(AttrDomain::Point, "heat")
                .is_none()
        );
        let prim_col = prim_avg
            .domain_attribute(AttrDomain::Prim, "heat")
            .and_then(AttributeColumn::as_f32);
        assert_eq!(
            prim_col.and_then(|c| c.get(0)).map(FiniteF32::get),
            Some(2.5)
        );
        assert_eq!(
            prim_col.and_then(|c| c.get(1)).map(FiniteF32::get),
            Some(6.75)
        );

        // Promote Prim -> Detail with Sum: 2.5 + 6.75 = 9.25
        let Ok(det_sum) = prim_avg.promote_attribute(
            "heat",
            AttrDomain::Prim,
            AttrDomain::Detail,
            PromoteMode::Sum,
        ) else {
            return;
        };
        let det_col = det_sum
            .domain_attribute(AttrDomain::Detail, "heat")
            .and_then(AttributeColumn::as_f32);
        assert_eq!(
            det_col.and_then(|c| c.get(0)).map(FiniteF32::get),
            Some(9.25)
        );

        // Promote Detail -> Point (broadcasts 9.25 to all 6 points)
        let Ok(back_to_pt) = det_sum.promote_attribute(
            "heat",
            AttrDomain::Detail,
            AttrDomain::Point,
            PromoteMode::Max,
        ) else {
            return;
        };
        let pt_col = back_to_pt
            .domain_attribute(AttrDomain::Point, "heat")
            .and_then(AttributeColumn::as_f32);
        assert_eq!(pt_col.map(PagedColumn::len), Some(6));
        assert_eq!(
            pt_col.and_then(|c| c.get(5)).map(FiniteF32::get),
            Some(9.25)
        );

        // Group right-hand quad by bounds [0.9, -0.1, -0.1]..[2.1, 0.1, 1.1] and blast it
        let Ok(grouped) = back_to_pt.group_by_bounds(
            GroupDomain::Prim,
            "right_quad",
            Vec3::checked(0.9, -0.1, -0.1).unwrap_or(Vec3::ZERO),
            Vec3::checked(2.1, 0.1, 1.1).unwrap_or(Vec3::ZERO),
        ) else {
            return;
        };
        let Ok(blasted) = grouped.blast_group("right_quad", false) else {
            return;
        };
        // Deleting quad 1 leaves quad 0 with 4 compacted points and 4 vertices
        assert_eq!(blasted.len(), 4);
        assert_eq!(blasted.prim_count(), 1);
        assert_eq!(blasted.vertex_count(), 4);
        assert!(blasted.topology_index().is_manifold());
    }

    #[test]
    fn uv_project_modes_normalize_coordinates_and_preserve_cow() {
        let pts = vec![
            Vec3::checked(0.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(4.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(4.0, 2.0, 8.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(0.0, 2.0, 8.0).unwrap_or(Vec3::ZERO),
        ];
        let base = GeoDetail::points(pts);
        for mode in [
            UvProjectMode::PlanarY,
            UvProjectMode::PlanarZ,
            UvProjectMode::PlanarX,
            UvProjectMode::Cylindrical,
            UvProjectMode::Spherical,
            UvProjectMode::Box,
        ] {
            let projected = base.uv_project(mode, 1 << 20);
            assert!(projected.is_ok());
            if let Ok(geo) = projected {
                assert!(geo.page_shared_with(&base, 0));
                let uv_col = geo.attribute("uv").and_then(AttributeColumn::as_vec3);
                assert!(uv_col.is_some());
                if let Some(col) = uv_col {
                    assert_eq!(col.len(), 4);
                    for i in 0..4 {
                        if let Some(uv) = col.get(i) {
                            assert!(uv.x >= 0.0 && uv.x <= 1.0);
                            assert!(uv.y >= 0.0 && uv.y <= 1.0);
                            assert_eq!(uv.z, 0.0);
                        }
                    }
                    if mode == UvProjectMode::PlanarY {
                        assert_eq!(col.get(0), Some(Vec3::ZERO));
                        assert_eq!(
                            col.get(2),
                            Some(Vec3::checked(1.0, 1.0, 0.0).unwrap_or(Vec3::ZERO))
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn pinched_vertices_are_counted_even_when_every_edge_is_manifold() -> Result<(), GeoError> {
        let v = |x, y, z| Vec3::checked(x, y, z);
        // Two tetrahedra sharing only point 0.
        let pinched = GeoDetail::points(vec![
            v(0.0, 0.0, 0.0)?,
            v(1.0, 0.0, 0.0)?,
            v(0.0, 1.0, 0.0)?,
            v(0.0, 0.0, 1.0)?,
            v(-1.0, 0.0, 0.0)?,
            v(0.0, -1.0, 0.0)?,
            v(0.0, 0.0, -1.0)?,
        ])
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 8,
            }],
            vec![
                0, 2, 1, 0, 1, 3, 1, 2, 3, 0, 3, 2, 0, 5, 4, 0, 4, 6, 4, 5, 6, 0, 6, 5,
            ],
        )?;
        let topology = TopologyIndex::build(&pinched);
        assert!(topology.is_closed_manifold(), "edges alone look fine");
        assert_eq!(topology.non_manifold_vertex_count(&pinched), 1);

        // Three separate fans around one point count once, and a clean tetrahedron not at all.
        let clean = GeoDetail::points(vec![
            v(0.0, 0.0, 0.0)?,
            v(1.0, 0.0, 0.0)?,
            v(0.0, 1.0, 0.0)?,
            v(0.0, 0.0, 1.0)?,
        ])
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 4,
            }],
            vec![0, 2, 1, 0, 1, 3, 1, 2, 3, 0, 3, 2],
        )?;
        assert_eq!(
            TopologyIndex::build(&clean).non_manifold_vertex_count(&clean),
            0
        );
        Ok(())
    }

    #[test]
    fn merging_keeps_attributes_and_groups_from_both_sides() -> Result<(), GeoError> {
        let triangle = |x: f32| -> Result<GeoDetail, GeoError> {
            GeoDetail::points(vec![
                Vec3::checked(x, 0.0, 0.0)?,
                Vec3::checked(x + 1.0, 0.0, 0.0)?,
                Vec3::checked(x, 1.0, 0.0)?,
            ])
            .with_primitives(
                vec![PrimRun {
                    kind: PrimKind::Polygon,
                    arity: 3,
                    prim_count: 1,
                }],
                vec![0, 1, 2],
            )
        };
        let f32s = |values: Vec<f32>| AttributeColumn::f32_checked(values);
        let left = triangle(0.0)?
            .with_attribute("shared", f32s(vec![1.0, 2.0, 3.0])?)?
            .with_attribute("left_only", AttributeColumn::i32_values(vec![7, 8, 9]))?
            .with_domain_attribute(
                AttrDomain::Prim,
                "piece",
                AttributeColumn::i32_values(vec![4]),
            )?
            .with_domain_attribute(
                AttrDomain::Vertex,
                "uv",
                AttributeColumn::vec3_values(vec![Vec3::UP; 3]),
            )?
            .with_domain_attribute(
                AttrDomain::Detail,
                "note",
                AttributeColumn::i32_values(vec![1]),
            )?
            .with_group(GroupDomain::Point, "picked", &[1])?;
        let right = triangle(5.0)?
            .with_attribute("shared", f32s(vec![4.0, 5.0, 6.0])?)?
            .with_attribute("right_only", AttributeColumn::bool_values(vec![true; 3]))?
            .with_domain_attribute(
                AttrDomain::Prim,
                "piece",
                AttributeColumn::i32_values(vec![6]),
            )?
            .with_domain_attribute(
                AttrDomain::Detail,
                "note",
                AttributeColumn::i32_values(vec![2]),
            )?
            .with_domain_attribute(
                AttrDomain::Detail,
                "extra",
                AttributeColumn::i32_values(vec![3]),
            )?
            .with_group(GroupDomain::Prim, "hero", &[0])?;
        let both = left.merged(&right, usize::MAX)?;
        let shared = both.attribute("shared").and_then(AttributeColumn::as_f32);
        let read = (0..6).map(|i| shared.and_then(|c| c.get(i)).map(|f| f.get()));
        assert_eq!(
            read.collect::<Vec<_>>(),
            [1., 2., 3., 4., 5., 6.].map(Some).to_vec()
        );
        // Elements from the side lacking an attribute read as zero.
        let only = both
            .attribute("left_only")
            .and_then(AttributeColumn::as_i32);
        assert_eq!(only.and_then(|c| c.get(1)), Some(8));
        assert_eq!(only.and_then(|c| c.get(4)), Some(0));
        let only = both
            .attribute("right_only")
            .and_then(AttributeColumn::as_bool);
        assert_eq!(only.and_then(|c| c.get(1)), Some(false));
        assert_eq!(only.and_then(|c| c.get(4)), Some(true));
        let piece = both
            .domain_attribute(AttrDomain::Prim, "piece")
            .and_then(AttributeColumn::as_i32);
        assert_eq!(
            (piece.and_then(|c| c.get(0)), piece.and_then(|c| c.get(1))),
            (Some(4), Some(6))
        );
        let uv = both
            .domain_attribute(AttrDomain::Vertex, "uv")
            .and_then(AttributeColumn::as_vec3);
        assert_eq!(uv.map(PagedColumn::len), Some(6));
        assert_eq!(uv.and_then(|c| c.get(1)), Some(Vec3::UP));
        assert_eq!(uv.and_then(|c| c.get(4)), Some(Vec3::ZERO));
        // Detail values come from the first side that has them.
        let detail = |name| {
            both.domain_attribute(AttrDomain::Detail, name)
                .and_then(AttributeColumn::as_i32)
                .and_then(|c| c.get(0))
        };
        assert_eq!((detail("note"), detail("extra")), (Some(1), Some(3)));
        let picked = both.group(GroupDomain::Point, "picked");
        assert_eq!(picked.map(ElementGroup::selected_count), Some(1));
        assert_eq!(
            picked.map(|g| (g.contains(1), g.contains(4))),
            Some((true, false))
        );
        let hero = both.group(GroupDomain::Prim, "hero");
        assert_eq!(
            hero.map(|g| (g.contains(0), g.contains(1))),
            Some((false, true))
        );

        // One name with two types is an error, not a silent choice.
        let conflict =
            triangle(0.0)?.with_attribute("shared", AttributeColumn::i32_values(vec![1; 3]))?;
        assert_eq!(
            left.merged(&conflict, usize::MAX).err(),
            Some(GeoError::AttributeTypeMismatch)
        );
        // Merging plain points keeps their attributes too.
        let cloud = GeoDetail::points(vec![Vec3::ZERO]).with_attribute("mass", f32s(vec![2.5])?)?;
        let clouds = cloud.merged(&cloud, usize::MAX)?;
        let mass = clouds.attribute("mass").and_then(AttributeColumn::as_f32);
        assert_eq!(mass.and_then(|c| c.get(1)).map(|f| f.get()), Some(2.5));
        Ok(())
    }
}
