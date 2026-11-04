//! Pure-Rust geometry serialization and interchange (`.ofxg` native binary cache and Wavefront `.obj`).
//! Enforces finite-number validation, resource bounds, and BLAKE3 content integrity on ingress.

use pcc_geo::{AttrDomain, AttributeColumn, GeoDetail, GeoError, GroupDomain, PrimKind, PrimRun};
use pcc_math::Vec3;

const OFXG_MAGIC: &[u8; 8] = b"OFXG\x01\x00\x00\x00";
pub const DEFAULT_MAX_IO_ELEMENTS: usize = 10_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoFormatError {
    InvalidMagic,
    UnexpectedEof,
    TrailingBytes,
    ChecksumMismatch,
    ContentHashMismatch,
    InvalidUtf8,
    InvalidTypeTag,
    InvalidSyntax,
    ElementLimitExceeded,
    Geo(GeoError),
}

impl From<GeoError> for IoFormatError {
    fn from(value: GeoError) -> Self {
        Self::Geo(value)
    }
}

/// Serializes a `GeoDetail` into the canonical `.ofxg` v1 binary cache format.
#[must_use]
pub fn to_ofxg_bytes(geo: &GeoDetail) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(OFXG_MAGIC);
    payload.extend_from_slice(geo.hash().as_bytes());

    // 1. Positions
    write_u64(&mut payload, geo.len() as u64);
    for idx in 0..geo.len() {
        let pt = geo.point(idx).unwrap_or(Vec3::ZERO);
        write_f32(&mut payload, pt.x);
        write_f32(&mut payload, pt.y);
        write_f32(&mut payload, pt.z);
    }

    // 2. Primitive runs & vertex-to-point topology
    let runs = geo.prim_runs();
    write_u64(&mut payload, runs.len() as u64);
    for run in runs {
        let kind_tag: u8 = match run.kind {
            PrimKind::Polygon => 1,
            PrimKind::Polyline => 2,
        };
        payload.push(kind_tag);
        write_u32(&mut payload, run.arity);
        write_u64(&mut payload, run.prim_count as u64);
    }
    write_u64(&mut payload, geo.vertex_count() as u64);
    for vtx_idx in 0..geo.vertex_count() {
        let pt_idx = geo.vertex_point(vtx_idx).unwrap_or(0);
        write_u32(&mut payload, pt_idx);
    }

    // 3. Attributes across all 4 domains
    for domain in [
        AttrDomain::Point,
        AttrDomain::Vertex,
        AttrDomain::Prim,
        AttrDomain::Detail,
    ] {
        let names = geo.domain_attribute_names(domain);
        write_u64(&mut payload, names.len() as u64);
        for name in names {
            write_str(&mut payload, name);
            if let Some(col) = geo.domain_attribute(domain, name) {
                write_attribute_column(&mut payload, col);
            }
        }
    }

    // 4. Selection groups across Point and Prim domains
    for domain in [GroupDomain::Point, GroupDomain::Prim] {
        let names = geo.group_names(domain);
        write_u64(&mut payload, names.len() as u64);
        for name in names {
            write_str(&mut payload, name);
            if let Some(group) = geo.group(domain, name) {
                write_u64(&mut payload, group.selected_count() as u64);
                for idx in 0..group.len() {
                    if group.contains(idx) {
                        write_u64(&mut payload, idx as u64);
                    }
                }
            }
        }
    }

    // 5. Append 32-byte BLAKE3 payload integrity digest
    let checksum = blake3::hash(&payload);
    payload.extend_from_slice(checksum.as_bytes());
    payload
}

/// Deserializes and validates a `GeoDetail` from `.ofxg` v1 bytes using `DEFAULT_MAX_IO_ELEMENTS`.
pub fn from_ofxg_bytes(bytes: &[u8]) -> Result<GeoDetail, IoFormatError> {
    from_ofxg_bytes_with_limit(bytes, DEFAULT_MAX_IO_ELEMENTS)
}

/// Deserializes and validates a `GeoDetail` from `.ofxg` v1 bytes with an explicit element limit.
pub fn from_ofxg_bytes_with_limit(
    bytes: &[u8],
    max_elements: usize,
) -> Result<GeoDetail, IoFormatError> {
    if bytes.len() < OFXG_MAGIC.len() + 32 + 32 {
        return Err(IoFormatError::UnexpectedEof);
    }
    let (body, footer_checksum) = bytes.split_at(bytes.len() - 32);
    if &body[..OFXG_MAGIC.len()] != OFXG_MAGIC {
        return Err(IoFormatError::InvalidMagic);
    }
    let actual_checksum = blake3::hash(body);
    if actual_checksum.as_bytes().as_slice() != footer_checksum {
        return Err(IoFormatError::ChecksumMismatch);
    }

    let mut cursor = OFXG_MAGIC.len();
    let expected_hash_bytes = read_slice(body, &mut cursor, 32)?;

    // 1. Positions
    let point_count = read_bounded_len(body, &mut cursor, max_elements)?;
    let mut points = Vec::with_capacity(point_count);
    for _ in 0..point_count {
        let x = read_f32(body, &mut cursor)?;
        let y = read_f32(body, &mut cursor)?;
        let z = read_f32(body, &mut cursor)?;
        let pt = Vec3::checked(x, y, z).map_err(|_| IoFormatError::Geo(GeoError::NonFinite))?;
        points.push(pt);
    }
    let mut geo = GeoDetail::points(points);

    // 2. Primitive runs & vertex-to-point topology
    let run_count = read_bounded_len(body, &mut cursor, max_elements)?;
    let mut runs = Vec::with_capacity(run_count);
    for _ in 0..run_count {
        let kind_tag = read_u8(body, &mut cursor)?;
        let kind = match kind_tag {
            1 => PrimKind::Polygon,
            2 => PrimKind::Polyline,
            _ => return Err(IoFormatError::InvalidTypeTag),
        };
        let arity = read_u32(body, &mut cursor)?;
        let prim_count = read_bounded_len(body, &mut cursor, max_elements)?;
        runs.push(PrimRun {
            kind,
            arity,
            prim_count,
        });
    }
    let vertex_count = read_bounded_len(body, &mut cursor, max_elements)?;
    let mut vertex_points = Vec::with_capacity(vertex_count);
    for _ in 0..vertex_count {
        vertex_points.push(read_u32(body, &mut cursor)?);
    }
    if !runs.is_empty() || !vertex_points.is_empty() {
        geo = geo.with_primitives(runs, vertex_points)?;
    }

    // 3. Attributes across all 4 domains
    for domain in [
        AttrDomain::Point,
        AttrDomain::Vertex,
        AttrDomain::Prim,
        AttrDomain::Detail,
    ] {
        let attr_count = read_bounded_len(body, &mut cursor, max_elements)?;
        for _ in 0..attr_count {
            let name = read_str(body, &mut cursor, max_elements)?;
            let col = read_attribute_column(body, &mut cursor, max_elements)?;
            geo = geo.with_domain_attribute(domain, name, col)?;
        }
    }

    // 4. Selection groups across Point and Prim domains
    for domain in [GroupDomain::Point, GroupDomain::Prim] {
        let group_count = read_bounded_len(body, &mut cursor, max_elements)?;
        for _ in 0..group_count {
            let name = read_str(body, &mut cursor, max_elements)?;
            let sel_len = read_bounded_len(body, &mut cursor, max_elements)?;
            let mut indices = Vec::with_capacity(sel_len);
            for _ in 0..sel_len {
                indices.push(read_bounded_len(body, &mut cursor, max_elements)?);
            }
            geo = geo.with_group(domain, name, &indices)?;
        }
    }

    if cursor != body.len() {
        return Err(IoFormatError::TrailingBytes);
    }
    if geo.hash().as_bytes().as_slice() != expected_hash_bytes {
        return Err(IoFormatError::ContentHashMismatch);
    }
    Ok(geo)
}

/// Exports `GeoDetail` points, polygons, and polylines to canonical Wavefront `.obj` text.
#[must_use]
pub fn to_obj(geo: &GeoDetail) -> String {
    let canonical = |v: f32| if v == 0.0 { 0.0 } else { v };
    let mut out = String::from("# ofxg-obj v1\n");
    for idx in 0..geo.len() {
        let p = geo.point(idx).unwrap_or(Vec3::ZERO);
        out.push_str(&format!(
            "v {:e} {:e} {:e}\n",
            canonical(p.x),
            canonical(p.y),
            canonical(p.z)
        ));
    }
    let mut vtx_cursor = 0usize;
    for run in geo.prim_runs() {
        let prefix = match run.kind {
            PrimKind::Polygon => "f",
            PrimKind::Polyline => "l",
        };
        let arity = run.arity as usize;
        for _ in 0..run.prim_count {
            out.push_str(prefix);
            for offset in 0..arity {
                let pt_idx = geo.vertex_point(vtx_cursor + offset).unwrap_or(0);
                out.push_str(&format!(" {}", pt_idx + 1));
            }
            out.push('\n');
            vtx_cursor += arity;
        }
    }
    out
}

/// Parses a `GeoDetail` from Wavefront `.obj` text (`v`, `f`, `l` records).
pub fn from_obj(text: &str) -> Result<GeoDetail, IoFormatError> {
    if !text.is_ascii() || text.contains('\r') {
        return Err(IoFormatError::InvalidSyntax);
    }
    let mut points = Vec::new();
    let mut runs: Vec<PrimRun> = Vec::new();
    let mut vertex_points: Vec<u32> = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_ascii_whitespace();
        let Some(keyword) = parts.next() else {
            continue;
        };
        match keyword {
            "v" => {
                let x = parse_f32_obj(parts.next())?;
                let y = parse_f32_obj(parts.next())?;
                let z = parse_f32_obj(parts.next())?;
                if parts.next().is_some() {
                    return Err(IoFormatError::InvalidSyntax);
                }
                let pt =
                    Vec3::checked(x, y, z).map_err(|_| IoFormatError::Geo(GeoError::NonFinite))?;
                points.push(pt);
            }
            "f" | "l" => {
                let kind = if keyword == "f" {
                    PrimKind::Polygon
                } else {
                    PrimKind::Polyline
                };
                let mut indices = Vec::new();
                for token in parts {
                    let head = token.split('/').next().unwrap_or("");
                    let one_based = head
                        .parse::<u32>()
                        .map_err(|_| IoFormatError::InvalidSyntax)?;
                    if one_based == 0 {
                        return Err(IoFormatError::Geo(GeoError::IndexOutOfBounds));
                    }
                    indices.push(one_based - 1);
                }
                let min_arity = match kind {
                    PrimKind::Polygon => 3,
                    PrimKind::Polyline => 2,
                };
                if indices.len() < min_arity {
                    return Err(IoFormatError::Geo(GeoError::InvalidTopology));
                }
                let arity = u32::try_from(indices.len())
                    .map_err(|_| IoFormatError::ElementLimitExceeded)?;
                vertex_points.extend_from_slice(&indices);
                if let Some(last) = runs.last_mut()
                    && last.kind == kind
                    && last.arity == arity
                {
                    last.prim_count += 1;
                } else {
                    runs.push(PrimRun {
                        kind,
                        arity,
                        prim_count: 1,
                    });
                }
            }
            _ => return Err(IoFormatError::InvalidSyntax),
        }
    }

    let geo = GeoDetail::points(points);
    if runs.is_empty() {
        Ok(geo)
    } else {
        Ok(geo.with_primitives(runs, vertex_points)?)
    }
}

fn parse_f32_obj(token: Option<&str>) -> Result<f32, IoFormatError> {
    let raw = token.ok_or(IoFormatError::InvalidSyntax)?;
    let val = raw
        .parse::<f32>()
        .map_err(|_| IoFormatError::InvalidSyntax)?;
    if val.is_finite() {
        Ok(val)
    } else {
        Err(IoFormatError::Geo(GeoError::NonFinite))
    }
}

fn write_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn write_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn write_f32(buf: &mut Vec<u8>, value: f32) {
    let canonical = if value == 0.0 { 0.0 } else { value };
    buf.extend_from_slice(&canonical.to_bits().to_le_bytes());
}

fn write_str(buf: &mut Vec<u8>, value: &str) {
    write_u64(buf, value.len() as u64);
    buf.extend_from_slice(value.as_bytes());
}

fn write_attribute_column(buf: &mut Vec<u8>, col: &AttributeColumn) {
    if let Some(f32_col) = col.as_f32() {
        buf.push(1);
        write_u64(buf, f32_col.len() as u64);
        for i in 0..f32_col.len() {
            let v = f32_col.get(i).map_or(0.0, pcc_geo::FiniteF32::get);
            write_f32(buf, v);
        }
    } else if let Some(i32_col) = col.as_i32() {
        buf.push(2);
        write_u64(buf, i32_col.len() as u64);
        for i in 0..i32_col.len() {
            buf.extend_from_slice(&i32_col.get(i).unwrap_or(0).to_le_bytes());
        }
    } else if let Some(u32_col) = col.as_u32() {
        buf.push(3);
        write_u64(buf, u32_col.len() as u64);
        for i in 0..u32_col.len() {
            write_u32(buf, u32_col.get(i).unwrap_or(0));
        }
    } else if let Some(bool_col) = col.as_bool() {
        buf.push(4);
        write_u64(buf, bool_col.len() as u64);
        for i in 0..bool_col.len() {
            buf.push(u8::from(bool_col.get(i).unwrap_or(false)));
        }
    } else if let Some(vec3_col) = col.as_vec3() {
        buf.push(5);
        write_u64(buf, vec3_col.len() as u64);
        for i in 0..vec3_col.len() {
            let v = vec3_col.get(i).unwrap_or(Vec3::ZERO);
            write_f32(buf, v.x);
            write_f32(buf, v.y);
            write_f32(buf, v.z);
        }
    }
}

fn read_slice<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    len: usize,
) -> Result<&'a [u8], IoFormatError> {
    let end = cursor
        .checked_add(len)
        .ok_or(IoFormatError::UnexpectedEof)?;
    if end > bytes.len() {
        return Err(IoFormatError::UnexpectedEof);
    }
    let slice = &bytes[*cursor..end];
    *cursor = end;
    Ok(slice)
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8, IoFormatError> {
    let s = read_slice(bytes, cursor, 1)?;
    Ok(s[0])
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, IoFormatError> {
    let s = read_slice(bytes, cursor, 4)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_i32(bytes: &[u8], cursor: &mut usize) -> Result<i32, IoFormatError> {
    let s = read_slice(bytes, cursor, 4)?;
    Ok(i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, IoFormatError> {
    let s = read_slice(bytes, cursor, 8)?;
    Ok(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

fn read_bounded_len(
    bytes: &[u8],
    cursor: &mut usize,
    max_elements: usize,
) -> Result<usize, IoFormatError> {
    let raw = read_u64(bytes, cursor)?;
    let len = usize::try_from(raw).map_err(|_| IoFormatError::ElementLimitExceeded)?;
    if len > max_elements {
        return Err(IoFormatError::ElementLimitExceeded);
    }
    Ok(len)
}

fn read_f32(bytes: &[u8], cursor: &mut usize) -> Result<f32, IoFormatError> {
    let bits = read_u32(bytes, cursor)?;
    let val = f32::from_bits(bits);
    if val.is_finite() {
        Ok(if val == 0.0 { 0.0 } else { val })
    } else {
        Err(IoFormatError::Geo(GeoError::NonFinite))
    }
}

fn read_str(
    bytes: &[u8],
    cursor: &mut usize,
    max_elements: usize,
) -> Result<String, IoFormatError> {
    let len = read_bounded_len(bytes, cursor, max_elements)?;
    let slice = read_slice(bytes, cursor, len)?;
    std::str::from_utf8(slice)
        .map(str::to_owned)
        .map_err(|_| IoFormatError::InvalidUtf8)
}

fn read_attribute_column(
    bytes: &[u8],
    cursor: &mut usize,
    max_elements: usize,
) -> Result<AttributeColumn, IoFormatError> {
    let tag = read_u8(bytes, cursor)?;
    let count = read_bounded_len(bytes, cursor, max_elements)?;
    match tag {
        1 => {
            let mut vals = Vec::with_capacity(count);
            for _ in 0..count {
                vals.push(read_f32(bytes, cursor)?);
            }
            Ok(AttributeColumn::f32_checked(vals)?)
        }
        2 => {
            let mut vals = Vec::with_capacity(count);
            for _ in 0..count {
                vals.push(read_i32(bytes, cursor)?);
            }
            Ok(AttributeColumn::i32_values(vals))
        }
        3 => {
            let mut vals = Vec::with_capacity(count);
            for _ in 0..count {
                vals.push(read_u32(bytes, cursor)?);
            }
            Ok(AttributeColumn::u32_values(vals))
        }
        4 => {
            let mut vals = Vec::with_capacity(count);
            for _ in 0..count {
                let b = read_u8(bytes, cursor)?;
                if b > 1 {
                    return Err(IoFormatError::InvalidTypeTag);
                }
                vals.push(b == 1);
            }
            Ok(AttributeColumn::bool_values(vals))
        }
        5 => {
            let mut vals = Vec::with_capacity(count);
            for _ in 0..count {
                let x = read_f32(bytes, cursor)?;
                let y = read_f32(bytes, cursor)?;
                let z = read_f32(bytes, cursor)?;
                let v =
                    Vec3::checked(x, y, z).map_err(|_| IoFormatError::Geo(GeoError::NonFinite))?;
                vals.push(v);
            }
            Ok(AttributeColumn::vec3_values(vals))
        }
        _ => Err(IoFormatError::InvalidTypeTag),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcc_math::Philox4x32;

    #[test]
    fn ofxg_and_obj_round_trip_preserve_hash_and_topology() {
        let base = GeoDetail::points(vec![
            Vec3::ZERO,
            Vec3::checked(1.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(1.0, 1.0, 0.0).unwrap_or(Vec3::ZERO),
            Vec3::checked(0.0, 1.0, 0.0).unwrap_or(Vec3::ZERO),
        ]);
        let mesh = base
            .with_primitives(
                vec![PrimRun {
                    kind: PrimKind::Polygon,
                    arity: 3,
                    prim_count: 2,
                }],
                vec![0, 1, 2, 0, 2, 3],
            )
            .and_then(|g| {
                let pscale = AttributeColumn::f32_checked(vec![0.5, 1.0, 1.5, 2.0])?;
                g.with_domain_attribute(AttrDomain::Point, "pscale", pscale)
            })
            .and_then(|g| {
                g.with_domain_attribute(
                    AttrDomain::Vertex,
                    "uv_idx",
                    AttributeColumn::u32_values(vec![0, 1, 2, 0, 2, 3]),
                )
            })
            .and_then(|g| {
                g.with_domain_attribute(
                    AttrDomain::Prim,
                    "part_id",
                    AttributeColumn::i32_values(vec![7, 8]),
                )
            })
            .and_then(|g| {
                g.with_domain_attribute(
                    AttrDomain::Detail,
                    "closed",
                    AttributeColumn::bool_values(vec![true]),
                )
            })
            .and_then(|g| g.with_group(GroupDomain::Point, "corners", &[0, 2]))
            .and_then(|g| g.with_group(GroupDomain::Prim, "upper", &[1]));
        assert!(mesh.is_ok());
        let Ok(mesh) = mesh else {
            return;
        };

        let encoded = to_ofxg_bytes(&mesh);
        let decoded = from_ofxg_bytes(&encoded);
        assert_eq!(decoded.as_ref().map(GeoDetail::hash), Ok(mesh.hash()));

        // Also test Wavefront `.obj` round-trip of points and polygon topology
        let base_mesh = base.with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 2,
            }],
            vec![0, 1, 2, 0, 2, 3],
        );
        assert!(base_mesh.is_ok());
        if let Ok(base_mesh) = base_mesh {
            let obj_text = to_obj(&base_mesh);
            let parsed_obj = from_obj(&obj_text);
            assert_eq!(
                parsed_obj.as_ref().map(GeoDetail::hash),
                Ok(base_mesh.hash())
            );
        }
    }

    #[test]
    fn property_and_fuzz_ofxg_rejects_corrupted_bytes_without_panic() {
        let rng = Philox4x32::new(0xCAFE_1234_5678_9ABC);
        for case_idx in 0..100u32 {
            let pt_count = ((rng.generate([case_idx, 0, 0, 0])[0] % 32) + 3) as usize;
            let mut pts = Vec::with_capacity(pt_count);
            for i in 0..pt_count {
                let u = rng.generate_unit_f32([case_idx, i as u32, 1, 0]);
                let pt = Vec3::checked(u[0] * 10.0, u[1] * 10.0, u[2] * 10.0).unwrap_or(Vec3::ZERO);
                pts.push(pt);
            }
            let geo = GeoDetail::points(pts);
            let bytes = to_ofxg_bytes(&geo);
            assert_eq!(from_ofxg_bytes(&bytes).map(|g| g.hash()), Ok(geo.hash()));

            // Flip a byte in `bytes` and verify checksum/magic rejection
            let mut corrupted = bytes.clone();
            let flip_idx = (rng.generate([case_idx, 0, 2, 0])[0] as usize) % corrupted.len();
            corrupted[flip_idx] ^= 0xFF;
            assert!(from_ofxg_bytes(&corrupted).is_err());
        }
    }
}
