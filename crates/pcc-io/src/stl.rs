//! STL import. Input is treated as untrusted: declared sizes are checked before allocating, and
//! non-finite coordinates are rejected. Facet normals are ignored, as they are often wrong in the
//! wild; orientation comes from vertex order alone.

use crate::IoFormatError;
use pcc_geo::{GeoDetail, GeoError, PrimKind, PrimRun};
use pcc_math::Vec3;
use std::collections::HashMap;

const BINARY_HEADER: usize = 80;
const BINARY_PREFIX: usize = BINARY_HEADER + 4;
const BINARY_FACET: usize = 50;

/// Welds exactly coincident vertices and assembles one triangle run. Degenerate triangles are kept:
/// they are part of the data, and operators must cope with them.
fn assemble(corners: &[Vec3]) -> Result<GeoDetail, IoFormatError> {
    let mut lookup: HashMap<[u32; 3], u32> = HashMap::with_capacity(corners.len() / 2);
    let mut points = Vec::new();
    let mut vertex_points = Vec::with_capacity(corners.len());
    for corner in corners {
        let key = [corner.x.to_bits(), corner.y.to_bits(), corner.z.to_bits()];
        let index = *lookup.entry(key).or_insert_with(|| {
            points.push(*corner);
            (points.len() - 1) as u32
        });
        vertex_points.push(index);
    }
    let prim_count = corners.len() / 3;
    let runs = if prim_count == 0 {
        Vec::new()
    } else {
        vec![PrimRun {
            kind: PrimKind::Polygon,
            arity: 3,
            prim_count,
        }]
    };
    Ok(GeoDetail::points(points).with_primitives(runs, vertex_points)?)
}

fn read_binary(bytes: &[u8], triangles: usize) -> Result<GeoDetail, IoFormatError> {
    let mut corners = Vec::with_capacity(triangles * 3);
    let facets = bytes
        .get(BINARY_PREFIX..)
        .ok_or(IoFormatError::UnexpectedEof)?
        .chunks_exact(BINARY_FACET)
        .take(triangles);
    for facet in facets {
        // Layout: normal (12 bytes), three vertices (36 bytes), attribute count (2 bytes).
        for vertex in facet[12..48].chunks_exact(12) {
            let component = |offset: usize| {
                f32::from_le_bytes([
                    vertex[offset],
                    vertex[offset + 1],
                    vertex[offset + 2],
                    vertex[offset + 3],
                ])
            };
            let point = Vec3::checked(component(0), component(4), component(8))
                .map_err(|_| IoFormatError::Geo(pcc_geo::GeoError::NonFinite))?;
            corners.push(point);
        }
    }
    assemble(&corners)
}

fn read_ascii(bytes: &[u8], max_triangles: usize) -> Result<GeoDetail, IoFormatError> {
    // Solid names may hold arbitrary bytes, which only `vertex` parsing below would reject.
    let text = String::from_utf8_lossy(bytes);
    let mut tokens = text.split_ascii_whitespace();
    let mut corners = Vec::new();
    while let Some(token) = tokens.next() {
        if !token.eq_ignore_ascii_case("vertex") {
            continue;
        }
        let mut coordinate = || -> Result<f32, IoFormatError> {
            tokens
                .next()
                .ok_or(IoFormatError::UnexpectedEof)?
                .parse::<f32>()
                .map_err(|_| IoFormatError::InvalidSyntax)
        };
        let (x, y, z) = (coordinate()?, coordinate()?, coordinate()?);
        let point =
            Vec3::checked(x, y, z).map_err(|_| IoFormatError::Geo(pcc_geo::GeoError::NonFinite))?;
        corners.push(point);
        if corners.len() / 3 > max_triangles {
            return Err(IoFormatError::ElementLimitExceeded);
        }
    }
    if !corners.len().is_multiple_of(3) {
        return Err(IoFormatError::InvalidSyntax);
    }
    assemble(&corners)
}

/// Writes binary STL. Every polygon must be a triangle; polylines cannot be represented and are
/// skipped. The header deliberately does not start with `solid`.
pub fn to_stl_bytes(geo: &GeoDetail) -> Result<Vec<u8>, IoFormatError> {
    let triangles: usize = geo
        .prim_runs()
        .iter()
        .filter(|run| run.kind == PrimKind::Polygon)
        .map(|run| {
            if run.arity == 3 {
                Ok(run.prim_count)
            } else {
                Err(IoFormatError::Geo(GeoError::InvalidTopology))
            }
        })
        .sum::<Result<usize, _>>()?;
    let count = u32::try_from(triangles).map_err(|_| IoFormatError::ElementLimitExceeded)?;
    let mut bytes = Vec::with_capacity(BINARY_PREFIX + triangles * BINARY_FACET);
    let mut header = [0u8; BINARY_HEADER];
    let label = b"pcc binary stl";
    header[..label.len()].copy_from_slice(label);
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&count.to_le_bytes());
    let mut cursor = 0usize;
    for run in geo.prim_runs() {
        let arity = run.arity as usize;
        if run.kind != PrimKind::Polygon {
            cursor += arity * run.prim_count;
            continue;
        }
        for _ in 0..run.prim_count {
            let mut corners = [Vec3::ZERO; 3];
            for (k, corner) in corners.iter_mut().enumerate() {
                let point = geo
                    .vertex_point(cursor + k)
                    .ok_or(GeoError::IndexOutOfBounds)?;
                *corner = geo
                    .point(point as usize)
                    .ok_or(GeoError::IndexOutOfBounds)?;
            }
            cursor += arity;
            let normal = corners[1]
                .sub_checked(corners[0])
                .and_then(|u| {
                    corners[2]
                        .sub_checked(corners[0])
                        .and_then(|v| u.cross_checked(v))
                })
                .map_or(Vec3::UP, Vec3::normalized_or_up);
            for value in [normal, corners[0], corners[1], corners[2]] {
                for component in [value.x, value.y, value.z] {
                    bytes.extend_from_slice(&component.to_le_bytes());
                }
            }
            bytes.extend_from_slice(&[0u8; 2]);
        }
    }
    Ok(bytes)
}

/// Whether the bytes begin like an ASCII STL. Many binary files also start with `solid`, so callers
/// must try the exact binary size test first.
fn looks_ascii(bytes: &[u8]) -> bool {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    bytes
        .get(start..start + 5)
        .is_some_and(|word| word.eq_ignore_ascii_case(b"solid"))
}

/// Parses binary or ASCII STL with at most `max_triangles` triangles.
///
/// A file is binary when its length exactly matches its declared facet count, even if its header
/// starts with `solid`. Otherwise files starting with `solid` are parsed as ASCII. Remaining files
/// are binary, and bytes past the declared facets are ignored, as some exporters append them.
pub fn from_stl_bytes(bytes: &[u8], max_triangles: usize) -> Result<GeoDetail, IoFormatError> {
    let declared = bytes
        .get(BINARY_HEADER..BINARY_PREFIX)
        .map(|count| u32::from_le_bytes([count[0], count[1], count[2], count[3]]) as usize);
    let binary_length =
        declared.and_then(|n| n.checked_mul(BINARY_FACET)?.checked_add(BINARY_PREFIX));
    if let (Some(triangles), Some(length)) = (declared, binary_length)
        && length == bytes.len()
    {
        if triangles > max_triangles {
            return Err(IoFormatError::ElementLimitExceeded);
        }
        return read_binary(bytes, triangles);
    }
    if looks_ascii(bytes) {
        return read_ascii(bytes, max_triangles);
    }
    match (declared, binary_length) {
        (Some(triangles), Some(length)) if length < bytes.len() => {
            if triangles > max_triangles {
                return Err(IoFormatError::ElementLimitExceeded);
            }
            read_binary(bytes, triangles)
        }
        _ => Err(IoFormatError::UnexpectedEof),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcc_math::Philox4x32;

    const LIMIT: usize = 1 << 20;

    fn binary_stl(header: &[u8], triangles: &[[[f32; 3]; 3]]) -> Vec<u8> {
        let mut bytes = vec![0u8; BINARY_HEADER];
        bytes[..header.len()].copy_from_slice(header);
        bytes.extend_from_slice(&(triangles.len() as u32).to_le_bytes());
        for triangle in triangles {
            bytes.extend_from_slice(&[0u8; 12]);
            for vertex in triangle {
                for component in vertex {
                    bytes.extend_from_slice(&component.to_le_bytes());
                }
            }
            bytes.extend_from_slice(&[0u8; 2]);
        }
        bytes
    }

    const TWO_TRIANGLES: [[[f32; 3]; 3]; 2] = [
        [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
    ];

    fn assert_two_welded_triangles(geo: &GeoDetail) {
        assert_eq!(geo.prim_count(), 2);
        assert_eq!(geo.len(), 4, "shared corners must weld");
        assert_eq!(geo.vertex_count(), 6);
        assert!((geo.surface_area() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn binary_files_parse_and_weld_shared_corners() -> Result<(), IoFormatError> {
        let bytes = binary_stl(b"plain header", &TWO_TRIANGLES);
        assert_two_welded_triangles(&from_stl_bytes(&bytes, LIMIT)?);
        Ok(())
    }

    #[test]
    fn binary_files_whose_header_starts_with_solid_are_not_mistaken_for_ascii()
    -> Result<(), IoFormatError> {
        let bytes = binary_stl(b"solid exported by a popular tool", &TWO_TRIANGLES);
        assert_two_welded_triangles(&from_stl_bytes(&bytes, LIMIT)?);
        Ok(())
    }

    #[test]
    fn ascii_files_parse_across_line_endings_case_and_multiple_solids() -> Result<(), IoFormatError>
    {
        let facet = |a: &str, b: &str, c: &str| {
            format!(
                "facet normal 0 0 1\n outer loop\n  vertex {a}\n  vertex {b}\n  vertex {c}\n endloop\nendfacet\n"
            )
        };
        let unix = format!(
            "solid first\n{}endsolid first\nsolid second\n{}endsolid second\n",
            facet("0 0 0", "1 0 0", "0 1 0"),
            facet("1 0 0", "1 1 0", "0 1 0")
        );
        assert_two_welded_triangles(&from_stl_bytes(unix.as_bytes(), LIMIT)?);
        let windows = unix.replace('\n', "\r\n");
        assert_two_welded_triangles(&from_stl_bytes(windows.as_bytes(), LIMIT)?);
        let shouting = unix.to_ascii_uppercase();
        assert_two_welded_triangles(&from_stl_bytes(shouting.as_bytes(), LIMIT)?);
        // Scientific notation and a non-UTF-8 solid name.
        let mut exotic = b"solid \xff\xfe\n".to_vec();
        exotic.extend_from_slice(facet("0e0 0.0E0 -0", "1.0e+0 0 0", "0 1E0 0").as_bytes());
        exotic.extend_from_slice(facet("1 0 0", "1 1 0", "0 1 0").as_bytes());
        assert_two_welded_triangles(&from_stl_bytes(&exotic, LIMIT)?);
        Ok(())
    }

    #[test]
    fn negative_zero_welds_with_positive_zero_and_trailing_bytes_are_tolerated()
    -> Result<(), IoFormatError> {
        let triangles = [
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [-0.0, 1.0, -0.0]],
        ];
        let mut bytes = binary_stl(b"x", &triangles);
        assert_two_welded_triangles(&from_stl_bytes(&bytes, LIMIT)?);
        bytes.extend_from_slice(b"exporter trailer");
        assert_two_welded_triangles(&from_stl_bytes(&bytes, LIMIT)?);
        Ok(())
    }

    #[test]
    fn written_files_read_back_identically_and_quads_are_refused() -> Result<(), IoFormatError> {
        let original = from_stl_bytes(&binary_stl(b"x", &TWO_TRIANGLES), LIMIT)?;
        let written = to_stl_bytes(&original)?;
        assert!(!written.starts_with(b"solid"));
        assert_eq!(written.len(), BINARY_PREFIX + 2 * BINARY_FACET);
        // The stored normal of the first triangle faces +Z, from its vertex order.
        let normal_z = f32::from_le_bytes([written[92], written[93], written[94], written[95]]);
        assert!((normal_z - 1.0).abs() < 1e-6, "{normal_z}");
        assert_eq!(from_stl_bytes(&written, LIMIT)?.hash(), original.hash());
        let quad = GeoDetail::points(vec![Vec3::ZERO; 4]).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 4,
                prim_count: 1,
            }],
            vec![0, 1, 2, 3],
        )?;
        assert_eq!(
            to_stl_bytes(&quad).err(),
            Some(IoFormatError::Geo(GeoError::InvalidTopology))
        );
        Ok(())
    }

    #[test]
    fn hostile_sizes_and_values_are_rejected_before_allocating() {
        // Declares 4 billion facets in a tiny file: must fail, not allocate 150 GB.
        let mut lying = vec![0u8; BINARY_HEADER];
        lying.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            from_stl_bytes(&lying, usize::MAX).err(),
            Some(IoFormatError::UnexpectedEof)
        );
        let bytes = binary_stl(b"x", &TWO_TRIANGLES);
        assert_eq!(
            from_stl_bytes(&bytes, 1).err(),
            Some(IoFormatError::ElementLimitExceeded)
        );
        assert_eq!(
            from_stl_bytes(&bytes[..bytes.len() - 1], LIMIT).err(),
            Some(IoFormatError::UnexpectedEof)
        );
        assert_eq!(
            from_stl_bytes(b"", LIMIT).err(),
            Some(IoFormatError::UnexpectedEof)
        );
        let nan = binary_stl(
            b"x",
            &[[[f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]],
        );
        assert_eq!(
            from_stl_bytes(&nan, LIMIT).err(),
            Some(IoFormatError::Geo(GeoError::NonFinite))
        );
        // Corners that do not make whole triangles are a format error, not a topology error.
        assert_eq!(
            from_stl_bytes(b"solid s vertex 0 0 0 vertex 1 0 0", LIMIT).err(),
            Some(IoFormatError::InvalidSyntax)
        );
        for bad in [
            "solid s vertex 0 0",
            "solid s vertex a b c",
            "solid s vertex 0 0 0 vertex 1 0 0",
            "solid s vertex inf 0 0 vertex 1 0 0 vertex 0 1 0",
        ] {
            assert!(from_stl_bytes(bad.as_bytes(), LIMIT).is_err(), "{bad}");
        }
        let many = "vertex 0 0 0 ".repeat(9);
        assert_eq!(
            from_stl_bytes(format!("solid s {many}").as_bytes(), 2).err(),
            Some(IoFormatError::ElementLimitExceeded)
        );
    }

    #[test]
    fn empty_solids_are_valid_and_corrupted_input_never_panics() {
        let empty = from_stl_bytes(b"solid nothing\nendsolid nothing\n", LIMIT);
        assert_eq!(empty.map(|geo| geo.prim_count()).ok(), Some(0));
        let rng = Philox4x32::new(0xC0FFEE);
        let binary = binary_stl(b"x", &TWO_TRIANGLES);
        let ascii = b"solid s facet normal 0 0 1 outer loop vertex 0 0 0 vertex 1 0 0 vertex 0 1 0 endloop endfacet endsolid s".to_vec();
        for (seed, source) in [binary, ascii].iter().enumerate() {
            for round in 0..2000u32 {
                let mut mutated = source.clone();
                let r = rng.generate([seed as u32, round, 0, 0]);
                for k in 0..=(r[3] % 4) as usize {
                    let at = r[k] as usize % mutated.len();
                    mutated[at] = (r[(k + 1) % 4] >> 8) as u8;
                }
                mutated.truncate(1 + (r[0] as usize % (mutated.len() + 1)).max(mutated.len() / 2));
                // Any result is acceptable as long as it returns: Ok geometry must be finite.
                if let Ok(geo) = from_stl_bytes(&mutated, LIMIT) {
                    assert!(geo.positions().get(0).is_none_or(|p| p.x.is_finite()));
                }
            }
        }
    }
}
