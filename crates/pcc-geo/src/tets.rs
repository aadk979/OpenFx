//! Tetrahedra: solid elements for soft-body simulation, stored as runs of four vertices.
//!
//! A tetrahedron `(a, b, c, d)` is positive when `(b - a) x (c - a) . (d - a) > 0`, and its outward
//! faces are then `FACES`. Everything else treats these as opaque, so an operator that only knows
//! surfaces must refuse them rather than skip them.

use crate::{AttrDomain, GeoDetail, GeoError, PAGE_LEN, PrimKind, PrimRun};
use pcc_math::Vec3;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

/// The four outward faces of a positive tetrahedron, wound like the rest of the engine's solids.
const FACES: [[usize; 3]; 4] = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]];

/// Six times the volume would be the determinant, so this is exact up to `f64` rounding.
fn signed_volume([a, b, c, d]: [Vec3; 4]) -> f64 {
    let edge = |to: Vec3| {
        [
            f64::from(to.x) - f64::from(a.x),
            f64::from(to.y) - f64::from(a.y),
            f64::from(to.z) - f64::from(a.z),
        ]
    };
    let (u, v, w) = (edge(b), edge(c), edge(d));
    let cross = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    (cross[0] * w[0] + cross[1] * w[1] + cross[2] * w[2]) / 6.0
}

/// A face seen from either side: its sorted corners, and which way round it was wound.
fn face_key([a, b, c]: [u32; 3]) -> ([u32; 3], bool) {
    let mut sorted = [a, b, c];
    sorted.sort_unstable();
    // Rotating a triangle keeps its winding, so start from the smallest corner to compare them.
    let (second, third) = if a == sorted[0] {
        (b, c)
    } else if b == sorted[0] {
        (c, a)
    } else {
        (a, b)
    };
    (sorted, second < third)
}

impl GeoDetail {
    /// How many tetrahedra this geometry holds.
    #[must_use]
    pub fn tet_count(&self) -> usize {
        self.prim_runs
            .iter()
            .filter(|run| run.kind == PrimKind::Tetrahedron)
            .map(|run| run.prim_count)
            .sum()
    }

    /// Point indices of every tetrahedron, in order.
    fn tet_corners(&self) -> Result<Vec<[u32; 4]>, GeoError> {
        let mut tets = Vec::with_capacity(self.tet_count());
        let mut cursor = 0usize;
        let vertex = |at: usize| self.vertex_point(at).ok_or(GeoError::IndexOutOfBounds);
        for run in &*self.prim_runs {
            let arity = run.arity as usize;
            if run.kind != PrimKind::Tetrahedron {
                cursor += arity * run.prim_count;
                continue;
            }
            for _ in 0..run.prim_count {
                tets.push([
                    vertex(cursor)?,
                    vertex(cursor + 1)?,
                    vertex(cursor + 2)?,
                    vertex(cursor + 3)?,
                ]);
                cursor += arity;
            }
        }
        Ok(tets)
    }

    fn tet_positions(&self, tet: [u32; 4]) -> Result<[Vec3; 4], GeoError> {
        let at = |p: u32| self.point(p as usize).ok_or(GeoError::IndexOutOfBounds);
        Ok([at(tet[0])?, at(tet[1])?, at(tet[2])?, at(tet[3])?])
    }

    /// Total signed volume of the tetrahedra. Inverted elements count against it.
    pub fn tet_volume(&self) -> Result<f64, GeoError> {
        let mut total = 0.0;
        for tet in self.tet_corners()? {
            total += signed_volume(self.tet_positions(tet)?);
        }
        Ok(total)
    }

    /// Extracts the outer surface: every face that exactly one tetrahedron uses, as outward-facing
    /// triangles, on only the points those faces use. Interior points are dropped along with their
    /// attributes.
    ///
    /// Refuses geometry whose surface would not mean anything: other kinds of primitive beside the
    /// tetrahedra, inverted or flat elements, and elements that overlap across a face.
    pub fn tet_surface_cancellable(
        &self,
        memory_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Self, GeoError> {
        if self
            .prim_runs
            .iter()
            .any(|run| run.kind != PrimKind::Tetrahedron)
        {
            return Err(GeoError::InvalidTopology);
        }
        let tets = self.tet_corners()?;
        let bytes = tets
            .len()
            .checked_mul(FACES.len() * std::mem::size_of::<[u32; 3]>())
            .ok_or(GeoError::LimitExceeded)?;
        if bytes > memory_limit {
            return Err(GeoError::LimitExceeded);
        }
        // Every face, wound outward from its tetrahedron.
        let outward =
            |tet: &[u32; 4], face: &[usize; 3]| [tet[face[0]], tet[face[1]], tet[face[2]]];
        let mut uses: BTreeMap<[u32; 3], (u32, bool)> = BTreeMap::new();
        for (index, tet) in tets.iter().enumerate() {
            if index.is_multiple_of(PAGE_LEN) && cancelled.load(Ordering::Acquire) {
                return Err(GeoError::Cancelled);
            }
            if signed_volume(self.tet_positions(*tet)?) <= 0.0 {
                return Err(GeoError::InvalidTopology);
            }
            for face in &FACES {
                let (key, wound) = face_key(outward(tet, face));
                let entry = uses.entry(key).or_insert((0, wound));
                entry.0 += 1;
                // Neighbours see a shared face from opposite sides. Anything else overlaps.
                if entry.0 > 2 || (entry.0 == 2 && entry.1 == wound) {
                    return Err(GeoError::InvalidTopology);
                }
            }
        }
        let mut vertices = Vec::new();
        for tet in &tets {
            for face in &FACES {
                let corners = outward(tet, face);
                if uses
                    .get(&face_key(corners).0)
                    .is_some_and(|use_| use_.0 == 1)
                {
                    vertices.extend_from_slice(&corners);
                }
            }
        }
        // Keep only the points that the surface reaches, in their original order.
        let mut used = vec![false; self.len()];
        for &point in &vertices {
            *used
                .get_mut(point as usize)
                .ok_or(GeoError::IndexOutOfBounds)? = true;
        }
        let mut remap = vec![0u32; self.len()];
        let mut kept = Vec::new();
        for (point, _) in used.iter().enumerate().filter(|(_, used)| **used) {
            remap[point] = kept.len() as u32;
            kept.push(point);
        }
        let positions = kept
            .iter()
            .map(|&p| self.point(p).ok_or(GeoError::IndexOutOfBounds))
            .collect::<Result<Vec<_>, _>>()?;
        let faces = vertices.len() / 3;
        let runs = if faces == 0 {
            Vec::new()
        } else {
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: faces,
            }]
        };
        let vertices = vertices.into_iter().map(|p| remap[p as usize]).collect();
        let mut surface = Self::points(positions).with_primitives(runs, vertices)?;
        for name in self.domain_attribute_names(AttrDomain::Point) {
            if let Some(column) = self.domain_attribute(AttrDomain::Point, name) {
                surface = surface.with_domain_attribute(
                    AttrDomain::Point,
                    name,
                    column.selected(&kept)?,
                )?;
            }
        }
        for name in self.domain_attribute_names(AttrDomain::Detail) {
            if let Some(column) = self.domain_attribute(AttrDomain::Detail, name) {
                surface =
                    surface.with_domain_attribute(AttrDomain::Detail, name, column.clone())?;
            }
        }
        Ok(surface)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AttributeColumn;

    fn v(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    fn tets(count: usize) -> Vec<PrimRun> {
        vec![PrimRun {
            kind: PrimKind::Tetrahedron,
            arity: 4,
            prim_count: count,
        }]
    }

    /// A tetrahedron with volume 3: sides 1, 2 and 9 along the axes.
    fn single() -> Result<GeoDetail, GeoError> {
        GeoDetail::points(vec![
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(0.0, 2.0, 0.0),
            v(0.0, 0.0, 9.0),
        ])
        .with_primitives(tets(1), vec![0, 1, 2, 3])
    }

    fn surface_of(geo: &GeoDetail) -> Result<GeoDetail, GeoError> {
        geo.tet_surface_cancellable(usize::MAX, &AtomicBool::new(false))
    }

    /// One tetrahedron cut into four around an interior point, which is the last point.
    fn split_around_center() -> Result<GeoDetail, GeoError> {
        GeoDetail::points(vec![
            v(0.0, 0.0, 0.0),
            v(4.0, 0.0, 0.0),
            v(0.0, 4.0, 0.0),
            v(0.0, 0.0, 4.0),
            v(1.0, 1.0, 1.0),
        ])
        .with_primitives(
            tets(4),
            vec![
                4, 1, 2, 3, // opposite corner 0
                0, 4, 2, 3, // opposite corner 1
                0, 1, 4, 3, // opposite corner 2
                0, 1, 2, 4, // opposite corner 3
            ],
        )
    }

    #[test]
    fn only_four_corners_make_a_tetrahedron() -> Result<(), GeoError> {
        let cloud = GeoDetail::points(vec![Vec3::ZERO; 5]);
        for arity in [3, 5] {
            let run = PrimRun {
                kind: PrimKind::Tetrahedron,
                arity,
                prim_count: 1,
            };
            let vertices = (0..arity).collect();
            assert_eq!(
                cloud.with_primitives(vec![run], vertices).err(),
                Some(GeoError::InvalidTopology),
                "arity {arity}"
            );
        }
        assert_eq!(single()?.tet_count(), 1);
        assert_eq!(cloud.tet_count(), 0);
        Ok(())
    }

    #[test]
    fn volumes_add_up_and_count_inverted_elements_against_the_total() -> Result<(), GeoError> {
        assert!((single()?.tet_volume()? - 3.0).abs() < 1e-12);
        // Cutting a solid into pieces must not change how much of it there is.
        let whole = 4.0f64 * 4.0 * 4.0 / 6.0;
        let split = split_around_center()?;
        assert_eq!(split.tet_count(), 4);
        assert!((split.tet_volume()? - whole).abs() < 1e-9);
        let inverted = single()?.with_primitives(tets(1), vec![0, 2, 1, 3])?;
        assert!((inverted.tet_volume()? + 3.0).abs() < 1e-12);
        // Skips other primitives, wherever they sit, without losing its place.
        let mixed = GeoDetail::points(vec![
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(0.0, 2.0, 0.0),
            v(0.0, 0.0, 9.0),
        ])
        .with_primitives(
            vec![
                PrimRun {
                    kind: PrimKind::Polygon,
                    arity: 3,
                    prim_count: 1,
                },
                tets(1)[0],
            ],
            vec![0, 1, 2, 0, 1, 2, 3],
        )?;
        assert!((mixed.tet_volume()? - 3.0).abs() < 1e-12);
        Ok(())
    }

    #[test]
    fn a_surface_encloses_exactly_what_its_elements_do() -> Result<(), GeoError> {
        // The divergence theorem links these two independent calculations.
        for geo in [single()?, split_around_center()?] {
            let surface = surface_of(&geo)?;
            assert_eq!(surface.prim_count(), 4);
            assert_eq!(surface.len(), 4);
            assert!(surface.topology_index().is_closed_manifold());
            let enclosed = f64::from(surface.enclosed_volume());
            assert!((enclosed - geo.tet_volume()?).abs() < 1e-4, "{enclosed}");
        }
        Ok(())
    }

    #[test]
    fn interior_faces_and_points_are_dropped_and_attributes_follow_the_points()
    -> Result<(), GeoError> {
        let geo = split_around_center()?
            .with_attribute(
                "mass",
                AttributeColumn::f32_checked(vec![1.0, 2.0, 3.0, 4.0, 5.0])?,
            )?
            .with_domain_attribute(
                AttrDomain::Detail,
                "note",
                AttributeColumn::i32_values(vec![9]),
            )?;
        let surface = surface_of(&geo)?;
        // The interior point was last, so the corners keep their order and their values.
        assert_eq!(surface.point(3), Some(v(0.0, 0.0, 4.0)));
        let mass = surface
            .attribute("mass")
            .and_then(AttributeColumn::as_f32)
            .ok_or(GeoError::AttributeNotFound)?;
        assert_eq!(mass.len(), 4);
        assert_eq!(mass.get(3).map(|m| m.get()), Some(4.0));
        let note = surface
            .domain_attribute(AttrDomain::Detail, "note")
            .and_then(AttributeColumn::as_i32);
        assert_eq!(note.and_then(|c| c.get(0)), Some(9));
        Ok(())
    }

    #[test]
    fn surfaces_are_refused_when_they_would_mean_nothing() -> Result<(), GeoError> {
        let refused = Some(GeoError::InvalidTopology);
        let shape = single()?;
        // Inverted, and flat.
        let inverted = shape.with_primitives(tets(1), vec![0, 2, 1, 3])?;
        assert_eq!(surface_of(&inverted).err(), refused);
        let flat = GeoDetail::points(vec![
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(0.0, 1.0, 0.0),
            v(1.0, 1.0, 0.0),
        ])
        .with_primitives(tets(1), vec![0, 1, 2, 3])?;
        assert_eq!(surface_of(&flat).err(), refused);
        // The same element twice sees its faces from the same side.
        let doubled = shape.with_primitives(tets(2), vec![0, 1, 2, 3, 0, 1, 2, 3])?;
        assert_eq!(surface_of(&doubled).err(), refused);
        // Two elements on one side of a face, with different far corners.
        let stacked = GeoDetail::points(vec![
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(0.0, 1.0, 0.0),
            v(0.0, 0.0, 1.0),
            v(0.1, 0.1, 2.0),
        ])
        .with_primitives(tets(2), vec![0, 1, 2, 3, 0, 1, 2, 4])?;
        assert_eq!(surface_of(&stacked).err(), refused);
        // Three elements cannot all share one face: the third is what must be refused.
        let fan = GeoDetail::points(vec![
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(0.0, 1.0, 0.0),
            v(0.0, 0.0, 1.0),
            v(0.0, 0.0, -1.0),
            v(0.1, 0.1, 2.0),
        ])
        .with_primitives(tets(3), vec![0, 1, 2, 3, 0, 2, 1, 4, 0, 1, 2, 5])?;
        assert_eq!(surface_of(&fan).err(), refused);
        // Other primitives have no place in a solid's surface, and must not vanish silently.
        let polygon = PrimRun {
            kind: PrimKind::Polygon,
            arity: 3,
            prim_count: 1,
        };
        let beside = shape.with_primitives(vec![tets(1)[0], polygon], vec![0, 1, 2, 3, 0, 1, 2])?;
        assert_eq!(surface_of(&beside).err(), refused);
        Ok(())
    }

    #[test]
    fn two_elements_meeting_at_a_face_share_it_from_opposite_sides() -> Result<(), GeoError> {
        // One above the base triangle and one below it.
        let geo = GeoDetail::points(vec![
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(0.0, 1.0, 0.0),
            v(0.0, 0.0, 1.0),
            v(0.0, 0.0, -1.0),
        ])
        .with_primitives(tets(2), vec![0, 1, 2, 3, 0, 2, 1, 4])?;
        let surface = surface_of(&geo)?;
        assert_eq!((surface.len(), surface.prim_count()), (5, 6));
        assert!(surface.topology_index().is_closed_manifold());
        assert!((f64::from(surface.enclosed_volume()) - geo.tet_volume()?).abs() < 1e-5);
        Ok(())
    }

    #[test]
    fn surfaces_respect_budgets_and_cancellation() -> Result<(), GeoError> {
        let geo = single()?;
        let running = AtomicBool::new(false);
        // One element, four faces, three corners each: 48 bytes.
        assert_eq!(
            geo.tet_surface_cancellable(47, &running).err(),
            Some(GeoError::LimitExceeded)
        );
        assert!(geo.tet_surface_cancellable(48, &running).is_ok());
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            geo.tet_surface_cancellable(usize::MAX, &cancelled).err(),
            Some(GeoError::Cancelled)
        );
        // No elements means no surface, rather than an error.
        let empty = surface_of(&GeoDetail::points(vec![Vec3::ZERO]))?;
        assert_eq!((empty.len(), empty.prim_count()), (0, 0));
        Ok(())
    }

    #[test]
    fn reversing_winding_keeps_a_mirrored_solid_positive() -> Result<(), GeoError> {
        let mirrored = GeoDetail::points(vec![
            v(0.0, 0.0, 0.0),
            v(-1.0, 0.0, 0.0),
            v(0.0, 2.0, 0.0),
            v(0.0, 0.0, 9.0),
        ])
        .with_primitives(tets(1), vec![0, 1, 2, 3])?;
        assert!((mirrored.tet_volume()? + 3.0).abs() < 1e-12);
        let fixed = mirrored.with_reversed_winding()?;
        assert!((fixed.tet_volume()? - 3.0).abs() < 1e-12);
        // A swap of two corners, not a reversal of all four, which would be an even permutation.
        let order: Vec<_> = (0..4).map(|at| fixed.vertex_point(at)).collect();
        assert_eq!(order, [0, 2, 1, 3].map(Some).to_vec());
        Ok(())
    }

    #[test]
    fn a_tetrahedron_hashes_differently_from_the_same_corners_as_a_polyline() -> Result<(), GeoError>
    {
        let shape = single()?;
        let line = shape.with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polyline,
                arity: 4,
                prim_count: 1,
            }],
            vec![0, 1, 2, 3],
        )?;
        assert_ne!(shape.hash(), line.hash());
        Ok(())
    }
}
