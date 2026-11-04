//! Area-weighted surface scattering with optional minimum spacing, and kernel attribute transfer.

use crate::common::{
    D3, SopContext, add, cross, distance, length, normalized, scale, sub, to_d3, to_vec3,
};
use crate::modify::triangles_of;
use crate::spatial::PointGrid;
use pcc_geo::{AttrDomain, AttributeColumn, GeoDetail, GeoError};
use pcc_math::{Philox4x32, Vec3};

/// Parameters of [`scatter_surface`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceScatter {
    pub count: u32,
    pub seed: u64,
    /// Minimum pairwise spacing. When positive, up to `count` samples are kept by dart throwing,
    /// so the output may hold fewer points than requested.
    pub min_distance: f64,
}

/// Scatters points on polygon surfaces with density proportional to area, writing the source
/// triangle normal to `v@N`.
pub fn scatter_surface(
    geo: &GeoDetail,
    params: &SurfaceScatter,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if !(params.min_distance.is_finite() && params.min_distance >= 0.0) {
        return Err(GeoError::InvalidTopology);
    }
    let target = params.count as usize;
    ctx.check_output(target.checked_mul(2).ok_or(GeoError::LimitExceeded)?, 0)?;
    let (points, triangles) = triangles_of(geo)?;
    let mut cumulative = Vec::with_capacity(triangles.len());
    let mut total = 0.0;
    for t in &triangles {
        let ab = sub(points[t[1] as usize], points[t[0] as usize]);
        let ac = sub(points[t[2] as usize], points[t[0] as usize]);
        total += length(cross(ab, ac)) * 0.5;
        cumulative.push(total);
    }
    if total <= 0.0 || target == 0 {
        return Ok(GeoDetail::points(Vec::new()));
    }
    let rng = Philox4x32::new(params.seed);
    let attempts = if params.min_distance > 0.0 {
        target.saturating_mul(30)
    } else {
        target
    };
    let mut grid = (params.min_distance > 0.0).then(|| PointGrid::new(params.min_distance));
    let mut positions: Vec<D3> = Vec::with_capacity(target);
    let mut normals = Vec::with_capacity(target);
    for attempt in 0..attempts {
        ctx.poll_every(attempt)?;
        if positions.len() == target {
            break;
        }
        let u = rng.generate_unit_f32([attempt as u32, (attempt >> 32) as u32, 0, 0]);
        let pick = f64::from(u[0]) * total;
        let slot = cumulative
            .partition_point(|&c| c <= pick)
            .min(triangles.len() - 1);
        let t = triangles[slot];
        let (a, b, c) = (
            points[t[0] as usize],
            points[t[1] as usize],
            points[t[2] as usize],
        );
        // Uniform barycentric sampling (Osada et al. 2002).
        let root = f64::from(u[1]).sqrt();
        let (w0, w1, w2) = (
            1.0 - root,
            root * (1.0 - f64::from(u[2])),
            root * f64::from(u[2]),
        );
        let p = add(add(scale(a, w0), scale(b, w1)), scale(c, w2));
        if let Some(active) = grid.as_mut() {
            let mut blocked = false;
            active.for_neighbours(p, |other| {
                blocked = blocked || distance(p, positions[other as usize]) < params.min_distance;
            });
            if blocked {
                continue;
            }
            active.insert(positions.len() as u32, p);
        }
        positions.push(p);
        normals.push(to_vec3(
            normalized(cross(sub(b, a), sub(c, a))).unwrap_or([0.0, 1.0, 0.0]),
        )?);
    }
    let out_positions = positions
        .iter()
        .map(|&p| to_vec3(p))
        .collect::<Result<Vec<_>, _>>()?;
    GeoDetail::points(out_positions).with_attribute("N", AttributeColumn::vec3_values(normals))
}

/// Wendland C2 kernel on `[0, radius]`: smooth, compactly supported, strictly positive inside.
fn wendland(d: f64, radius: f64) -> f64 {
    let q = d / radius;
    let falloff = 1.0 - q;
    falloff * falloff * falloff * falloff * (4.0 * q + 1.0)
}

/// Transfers point attribute `name` (`F32` or `Vec3`) from `source` onto the points of `input` as
/// the normalized kernel average of source points within `radius`. Destination points with no
/// source in range keep their existing value, or zero when the attribute is new.
pub fn attrib_transfer(
    input: &GeoDetail,
    source: &GeoDetail,
    name: &str,
    radius: f64,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if !(radius.is_finite() && radius > 0.0) {
        return Err(GeoError::InvalidTopology);
    }
    let column = source.attribute(name).ok_or(GeoError::AttributeNotFound)?;
    let channel: Vec<D3> = match column {
        AttributeColumn::F32(values) => (0..values.len())
            .map(|i| [values.get(i).map_or(0.0, |f| f64::from(f.get())), 0.0, 0.0])
            .collect(),
        AttributeColumn::Vec3(values) => (0..values.len())
            .map(|i| values.get(i).map_or([0.0; 3], to_d3))
            .collect(),
        _ => return Err(GeoError::AttributeTypeMismatch),
    };
    let existing = input.attribute(name);
    let is_vec3 = matches!(column, AttributeColumn::Vec3(_));
    let previous: Option<Vec<D3>> = match existing {
        None => None,
        Some(AttributeColumn::F32(values)) if !is_vec3 => Some(
            (0..values.len())
                .map(|i| [values.get(i).map_or(0.0, |f| f64::from(f.get())), 0.0, 0.0])
                .collect(),
        ),
        Some(AttributeColumn::Vec3(values)) if is_vec3 => Some(
            (0..values.len())
                .map(|i| values.get(i).map_or([0.0; 3], to_d3))
                .collect(),
        ),
        Some(_) => return Err(GeoError::AttributeTypeMismatch),
    };
    ctx.check_output(input.len(), 0)?;
    let mut grid = PointGrid::new(radius);
    let source_points = crate::common::load_points(source)?;
    for (i, &p) in source_points.iter().enumerate() {
        ctx.poll_every(i)?;
        grid.insert(i as u32, p);
    }
    let mut result = Vec::with_capacity(input.len());
    for (i, p) in crate::common::load_points(input)?.into_iter().enumerate() {
        ctx.poll_every(i)?;
        let mut weighted = [0.0; 3];
        let mut weight_sum = 0.0;
        grid.for_neighbours(p, |candidate| {
            let d = distance(p, source_points[candidate as usize]);
            if d < radius {
                let w = wendland(d, radius);
                weighted = add(weighted, scale(channel[candidate as usize], w));
                weight_sum += w;
            }
        });
        result.push(if weight_sum > 0.0 {
            scale(weighted, 1.0 / weight_sum)
        } else {
            previous.as_ref().map_or([0.0; 3], |values| values[i])
        });
    }
    let out_column = if is_vec3 {
        let values: Result<Vec<Vec3>, GeoError> = result.iter().map(|&v| to_vec3(v)).collect();
        AttributeColumn::vec3_values(values?)
    } else {
        AttributeColumn::f32_checked(result.iter().map(|v| v[0] as f32).collect())?
    };
    input.with_domain_attribute(AttrDomain::Point, name, out_column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::create_sphere;
    use pcc_geo::{PrimKind, PrimRun};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn scatter_points_lie_on_the_surface_with_outward_normals_and_uniform_density()
    -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 26, &flag);
        let sphere = create_sphere(1.0, 64, 96, &ctx)?;
        let params = SurfaceScatter {
            count: 20_000,
            seed: 11,
            min_distance: 0.0,
        };
        let cloud = scatter_surface(&sphere, &params, &ctx)?;
        assert_eq!(cloud.len(), 20_000);
        let mut upper = 0usize;
        let normal_column = cloud
            .attribute("N")
            .and_then(AttributeColumn::as_vec3)
            .ok_or(GeoError::AttributeNotFound)?;
        for i in 0..cloud.len() {
            let p = to_d3(cloud.point(i).ok_or(GeoError::IndexOutOfBounds)?);
            let r = length(p);
            assert!((0.998..=1.0 + 1e-5).contains(&r), "radius {r}");
            let n = to_d3(normal_column.get(i).ok_or(GeoError::IndexOutOfBounds)?);
            assert!(crate::common::dot(p, n) > 0.99);
            if p[1] > 0.0 {
                upper += 1;
            }
        }
        // Binomial(20000, 0.5): six standard deviations is ~424 points.
        assert!(upper.abs_diff(10_000) < 424, "hemisphere balance {upper}");
        let again = scatter_surface(&sphere, &params, &ctx)?;
        assert_eq!(again.hash(), cloud.hash());
        Ok(())
    }

    #[test]
    fn min_distance_is_honoured_by_every_pair() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 26, &flag);
        let sphere = create_sphere(1.0, 32, 48, &ctx)?;
        let cloud = scatter_surface(
            &sphere,
            &SurfaceScatter {
                count: 800,
                seed: 3,
                min_distance: 0.12,
            },
            &ctx,
        )?;
        let points = crate::common::load_points(&cloud)?;
        assert!(points.len() > 300, "only {} samples", points.len());
        for (i, &a) in points.iter().enumerate() {
            for &b in &points[i + 1..] {
                assert!(distance(a, b) >= 0.12);
            }
        }
        Ok(())
    }

    #[test]
    fn transfer_reproduces_constant_and_linear_fields_and_keeps_out_of_range_values()
    -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 24, &flag);
        // A dense lattice carrying f(x) = 2x + 1: a symmetric kernel reproduces linear fields at
        // lattice-interior points up to the lattice's symmetry.
        let mut source_points = Vec::new();
        let mut values = Vec::new();
        for i in 0..21 {
            for j in 0..21 {
                let (x, y) = (i as f32 * 0.5, j as f32 * 0.5);
                source_points.push(Vec3::checked(x, y, 0.0)?);
                values.push(2.0 * x + 1.0);
            }
        }
        let source = GeoDetail::points(source_points)
            .with_attribute("heat", AttributeColumn::f32_checked(values)?)?;
        let input = GeoDetail::points(vec![
            Vec3::checked(5.0, 5.0, 0.0)?,
            Vec3::checked(100.0, 0.0, 0.0)?,
        ])
        .with_attribute("heat", AttributeColumn::f32_checked(vec![0.0, -7.0])?)?;
        let out = attrib_transfer(&input, &source, "heat", 1.6, &ctx)?;
        let read = |i| {
            out.attribute("heat")
                .and_then(AttributeColumn::as_f32)
                .and_then(|c| c.get(i))
                .map(|f| f.get())
        };
        let center = read(0).ok_or(GeoError::AttributeNotFound)?;
        assert!((center - 11.0).abs() < 1e-4, "center {center}");
        assert_eq!(read(1), Some(-7.0));
        assert_eq!(
            attrib_transfer(&input, &source, "missing", 1.0, &ctx).err(),
            Some(GeoError::AttributeNotFound)
        );
        Ok(())
    }
    fn triangle_soup(points: Vec<Vec3>, prims: usize) -> Result<GeoDetail, GeoError> {
        let vertices = (0..points.len() as u32).collect();
        GeoDetail::points(points).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: prims,
            }],
            vertices,
        )
    }

    #[test]
    fn density_is_uniform_within_a_triangle_and_proportional_to_area_across_them()
    -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 24, &flag);
        let params = SurfaceScatter {
            count: 20_000,
            seed: 17,
            min_distance: 0.0,
        };
        // x + y < 1/2 covers a quarter of this right triangle's area.
        let single = triangle_soup(
            vec![
                Vec3::checked(0.0, 0.0, 0.0)?,
                Vec3::checked(1.0, 0.0, 0.0)?,
                Vec3::checked(0.0, 1.0, 0.0)?,
            ],
            1,
        )?;
        let cloud = scatter_surface(&single, &params, &ctx)?;
        let near_corner = crate::common::load_points(&cloud)?
            .iter()
            .filter(|p| p[0] + p[1] < 0.5)
            .count();
        // Binomial(20000, 0.25): six standard deviations is ~368 points.
        assert!(
            near_corner.abs_diff(5_000) < 368,
            "near corner {near_corner}"
        );

        // The first triangle has nine times the area of the second, far away.
        let pair = triangle_soup(
            vec![
                Vec3::checked(0.0, 0.0, 0.0)?,
                Vec3::checked(3.0, 0.0, 0.0)?,
                Vec3::checked(0.0, 3.0, 0.0)?,
                Vec3::checked(10.0, 0.0, 0.0)?,
                Vec3::checked(11.0, 0.0, 0.0)?,
                Vec3::checked(10.0, 1.0, 0.0)?,
            ],
            2,
        )?;
        let cloud = scatter_surface(&pair, &params, &ctx)?;
        let on_large = crate::common::load_points(&cloud)?
            .iter()
            .filter(|p| p[0] < 5.0)
            .count();
        // Binomial(20000, 0.9): six standard deviations is ~255 points.
        assert!(on_large.abs_diff(18_000) < 255, "on large {on_large}");
        Ok(())
    }

    #[test]
    fn transfer_weights_follow_the_wendland_kernel_exactly() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        assert!((wendland(0.0, 2.0) - 1.0).abs() < 1e-12);
        assert!(wendland(2.0, 2.0).abs() < 1e-12);
        assert!((wendland(1.0, 2.0) - 0.1875).abs() < 1e-12);
        // Sources at q = 1/4 (value 1) and q = 1/2 (value 0) of the radius from the target:
        // the result is w(1/4) / (w(1/4) + w(1/2)) = 0.6328125 / 0.8203125.
        let source = GeoDetail::points(vec![
            Vec3::checked(0.25, 0.0, 0.0)?,
            Vec3::checked(-0.5, 0.0, 0.0)?,
        ])
        .with_attribute("mask", AttributeColumn::f32_checked(vec![1.0, 0.0])?)?;
        let input = GeoDetail::points(vec![Vec3::ZERO]);
        let out = attrib_transfer(&input, &source, "mask", 1.0, &ctx)?;
        let value = out
            .attribute("mask")
            .and_then(AttributeColumn::as_f32)
            .and_then(|c| c.get(0))
            .map(|f| f64::from(f.get()))
            .ok_or(GeoError::AttributeNotFound)?;
        assert!((value - 0.6328125 / 0.8203125).abs() < 1e-6, "{value}");
        Ok(())
    }
}
