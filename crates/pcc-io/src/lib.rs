//! Pure-Rust geometry serialization and interchange (`.ofxg` native binary cache, Wavefront `.obj`,
//! and STL import).
//! Enforces finite-number validation, resource bounds, and BLAKE3 content integrity on ingress.

mod stl;

use pcc_geo::{
    AttrDomain, AttributeColumn, BRICK_DIM, BRICK_LEN, Brick8x8x8, FiniteF32, GeoDetail, GeoError,
    GroupDomain, MAX_PACKED_DEPTH, PackedInstance, PrimKind, PrimRun, VdbGrid,
};
use pcc_math::Vec3;
use std::{collections::BTreeMap, sync::Arc};

pub use stl::{from_stl_bytes, to_stl_bytes};

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
            PrimKind::Packed => 3,
            PrimKind::Tetrahedron => 4,
            PrimKind::Volume => 5,
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

    // 5. Packed instances, only when present, so that other geometry's files never change.
    if geo.packed_count() > 0 {
        let prototypes = geo.packed_prototypes();
        write_u64(&mut payload, prototypes.len() as u64);
        for prototype in prototypes {
            let bytes = to_ofxg_bytes(prototype.geo());
            write_u64(&mut payload, bytes.len() as u64);
            payload.extend_from_slice(&bytes);
        }
        write_u64(&mut payload, geo.packed_count() as u64);
        for index in 0..geo.packed_count() {
            if let Some(instance) = geo.packed_instance(index) {
                write_u32(&mut payload, instance.prototype);
                for row in instance.rows {
                    write_f32(&mut payload, row.x);
                    write_f32(&mut payload, row.y);
                    write_f32(&mut payload, row.z);
                }
            }
        }
    }

    // 6. Volumes, only when present, for the same reason.
    if geo.volume_count() > 0 {
        write_u64(&mut payload, geo.volume_count() as u64);
        for index in 0..geo.volume_count() {
            if let Some(grid) = geo.volume(index) {
                write_volume_grid(&mut payload, grid);
            }
        }
    }

    // 7. Append 32-byte BLAKE3 payload integrity digest
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
    read_ofxg(bytes, max_elements, 0)
}

/// Reads a file, whose packed prototypes are themselves files. `depth` bounds that recursion, so a
/// crafted file cannot exhaust the stack by nesting.
fn read_ofxg(bytes: &[u8], max_elements: usize, depth: u32) -> Result<GeoDetail, IoFormatError> {
    if depth > MAX_PACKED_DEPTH {
        return Err(IoFormatError::ElementLimitExceeded);
    }
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
            3 => PrimKind::Packed,
            4 => PrimKind::Tetrahedron,
            5 => PrimKind::Volume,
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
        geo = geo.with_runs(runs, vertex_points)?;
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

    // 5. Packed instances, which only exist in files of geometry that has them.
    let packed_expected: usize = geo
        .prim_runs()
        .iter()
        .filter(|run| run.kind == PrimKind::Packed)
        .map(|run| run.prim_count)
        .sum();
    if packed_expected > 0 {
        let prototype_count = read_bounded_len(body, &mut cursor, max_elements)?;
        let mut prototypes = Vec::with_capacity(prototype_count);
        for _ in 0..prototype_count {
            let length = read_bounded_len(body, &mut cursor, usize::MAX)?;
            let nested = read_slice(body, &mut cursor, length)?;
            prototypes.push(Arc::new(read_ofxg(nested, max_elements, depth + 1)?));
        }
        let instance_count = read_bounded_len(body, &mut cursor, max_elements)?;
        let mut instances = Vec::with_capacity(instance_count);
        for _ in 0..instance_count {
            let prototype = read_u32(body, &mut cursor)?;
            let mut rows = [Vec3::ZERO; 3];
            for row in &mut rows {
                let x = read_f32(body, &mut cursor)?;
                let y = read_f32(body, &mut cursor)?;
                let z = read_f32(body, &mut cursor)?;
                *row =
                    Vec3::checked(x, y, z).map_err(|_| IoFormatError::Geo(GeoError::NonFinite))?;
            }
            instances.push(PackedInstance { prototype, rows });
        }
        geo = geo.with_packed_parts(prototypes, &instances)?;
    }

    // 6. Volumes, which likewise only exist in files of geometry that has them.
    let volumes_expected: usize = geo
        .prim_runs()
        .iter()
        .filter(|run| run.kind == PrimKind::Volume)
        .map(|run| run.prim_count)
        .sum();
    if volumes_expected > 0 {
        let grid_count = read_bounded_len(body, &mut cursor, max_elements)?;
        let mut grids = Vec::with_capacity(grid_count);
        for _ in 0..grid_count {
            grids.push(Arc::new(read_volume_grid(body, &mut cursor, max_elements)?));
        }
        geo = geo.with_volume_grids(grids)?;
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
    let texture = geo
        .domain_attribute(AttrDomain::Vertex, "uv")
        .and_then(AttributeColumn::as_vec3);
    if let Some(column) = texture {
        for idx in 0..column.len() {
            let uv = column.get(idx).unwrap_or(Vec3::ZERO);
            out.push_str(&format!("vt {:e} {:e}\n", canonical(uv.x), canonical(uv.y)));
        }
    }
    let mut vtx_cursor = 0usize;
    for run in geo.prim_runs() {
        let arity = run.arity as usize;
        let prefix = match run.kind {
            PrimKind::Polygon => "f",
            PrimKind::Polyline => "l",
            // OBJ has neither of these. Callers that must not lose them refuse to get here.
            PrimKind::Packed | PrimKind::Tetrahedron | PrimKind::Volume => {
                vtx_cursor += arity * run.prim_count;
                continue;
            }
        };
        for _ in 0..run.prim_count {
            out.push_str(prefix);
            for offset in 0..arity {
                let pt_idx = geo.vertex_point(vtx_cursor + offset).unwrap_or(0);
                if texture.is_some() {
                    out.push_str(&format!(" {}/{}", pt_idx + 1, vtx_cursor + offset + 1));
                } else {
                    out.push_str(&format!(" {}", pt_idx + 1));
                }
            }
            out.push('\n');
            vtx_cursor += arity;
        }
    }
    out
}

/// Parses one `v` or `vt` index of a face corner. OBJ indices are 1-based, and negative values
/// count back from the most recent element.
fn parse_obj_index(token: &str, count: usize) -> Result<u32, IoFormatError> {
    let value = token
        .parse::<i64>()
        .map_err(|_| IoFormatError::InvalidSyntax)?;
    let resolved = if value > 0 {
        value - 1
    } else if value < 0 {
        i64::try_from(count).map_err(|_| IoFormatError::ElementLimitExceeded)? + value
    } else {
        -1
    };
    u32::try_from(resolved).map_err(|_| IoFormatError::Geo(GeoError::IndexOutOfBounds))
}

/// Parses Wavefront `.obj` text as exported by common tools: `v`, `vt`, `f`, and `l` records with
/// any line ending. Vertex colours, normals, groups, and materials are ignored because the engine
/// derives or lacks them. Free-form surfaces fail loudly rather than vanish. Texture coordinates
/// become vertex `v@uv` only when every corner has one.
pub fn from_obj(text: &str) -> Result<GeoDetail, IoFormatError> {
    from_obj_with_limit(text, DEFAULT_MAX_IO_ELEMENTS)
}

/// [`from_obj`] with an explicit limit on points plus primitive corners.
pub fn from_obj_with_limit(text: &str, max_elements: usize) -> Result<GeoDetail, IoFormatError> {
    let mut points = Vec::new();
    let mut texture = Vec::new();
    let mut runs: Vec<PrimRun> = Vec::new();
    let mut vertex_points: Vec<u32> = Vec::new();
    let mut corner_texture: Vec<u32> = Vec::new();
    let mut all_textured = true;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_ascii_whitespace();
        let Some(keyword) = parts.next() else {
            continue;
        };
        if points.len() + vertex_points.len() + texture.len() > max_elements {
            return Err(IoFormatError::ElementLimitExceeded);
        }
        match keyword {
            "v" => {
                let x = parse_f32_obj(parts.next())?;
                let y = parse_f32_obj(parts.next())?;
                let z = parse_f32_obj(parts.next())?;
                // Allowed remainders: nothing, a weight, a colour, or a weight and a colour.
                let extra = parts.count();
                if !matches!(extra, 0 | 1 | 3 | 4) {
                    return Err(IoFormatError::InvalidSyntax);
                }
                let pt =
                    Vec3::checked(x, y, z).map_err(|_| IoFormatError::Geo(GeoError::NonFinite))?;
                points.push(pt);
            }
            "vt" => {
                let u = parse_f32_obj(parts.next())?;
                let v = match parts.next() {
                    Some(token) => parse_f32_obj(Some(token))?,
                    None => 0.0,
                };
                let uv = Vec3::checked(u, v, 0.0)
                    .map_err(|_| IoFormatError::Geo(GeoError::NonFinite))?;
                texture.push(uv);
            }
            "f" | "l" => {
                let kind = if keyword == "f" {
                    PrimKind::Polygon
                } else {
                    PrimKind::Polyline
                };
                let mut indices = Vec::new();
                for token in parts {
                    let mut fields = token.split('/');
                    let head = fields.next().unwrap_or("");
                    indices.push(parse_obj_index(head, points.len())?);
                    match fields.next() {
                        Some(field) if !field.is_empty() => {
                            corner_texture.push(parse_obj_index(field, texture.len())?);
                        }
                        _ => all_textured = false,
                    }
                }
                let min_arity = match kind {
                    PrimKind::Polygon => 3,
                    _ => 2,
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
            "vn" | "vp" | "g" | "o" | "s" | "mg" | "usemtl" | "mtllib" | "usemap" | "maplib" => {}
            _ => return Err(IoFormatError::InvalidSyntax),
        }
    }

    let geo = GeoDetail::points(points);
    if runs.is_empty() {
        return Ok(geo);
    }
    let geo = geo.with_primitives(runs, vertex_points)?;
    if !all_textured || corner_texture.is_empty() {
        return Ok(geo);
    }
    let mut uv = Vec::with_capacity(corner_texture.len());
    for &index in &corner_texture {
        uv.push(
            *texture
                .get(index as usize)
                .ok_or(IoFormatError::Geo(GeoError::IndexOutOfBounds))?,
        );
    }
    Ok(geo.with_domain_attribute(AttrDomain::Vertex, "uv", AttributeColumn::vec3_values(uv))?)
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

fn write_volume_grid(buf: &mut Vec<u8>, grid: &VdbGrid) {
    write_f32(buf, grid.voxel_size());
    write_f32(buf, grid.background());
    write_u64(buf, grid.active_brick_count() as u64);
    for (&(x, y, z), brick) in grid.bricks() {
        for coordinate in [x, y, z] {
            buf.extend_from_slice(&coordinate.to_le_bytes());
        }
        for value in brick.values() {
            write_f32(buf, value.get());
        }
    }
}

/// Brick coordinates beyond this could overflow a voxel index, in `i32`, once multiplied out.
const MAX_BRICK_COORDINATE: i32 = i32::MAX / (BRICK_DIM as i32) - 1;

/// Reads a grid, refusing anything `write_volume_grid` could not have written: bricks must be in
/// strictly ascending order, because two files for one grid would otherwise differ in hash.
fn read_volume_grid(
    bytes: &[u8],
    cursor: &mut usize,
    max_elements: usize,
) -> Result<VdbGrid, IoFormatError> {
    let voxel_size = FiniteF32::checked(read_f32(bytes, cursor)?).map_err(IoFormatError::Geo)?;
    let background = FiniteF32::checked(read_f32(bytes, cursor)?).map_err(IoFormatError::Geo)?;
    let brick_count = read_bounded_len(bytes, cursor, max_elements)?;
    let mut bricks = BTreeMap::new();
    let mut previous = None;
    for _ in 0..brick_count {
        let coordinate = (
            read_i32(bytes, cursor)?,
            read_i32(bytes, cursor)?,
            read_i32(bytes, cursor)?,
        );
        let (x, y, z) = coordinate;
        if [x, y, z]
            .iter()
            .any(|c| c.unsigned_abs() > MAX_BRICK_COORDINATE.unsigned_abs())
            || previous.is_some_and(|last| coordinate <= last)
        {
            return Err(IoFormatError::Geo(GeoError::InvalidTopology));
        }
        previous = Some(coordinate);
        let mut values = Vec::with_capacity(BRICK_LEN);
        for _ in 0..BRICK_LEN {
            values.push(FiniteF32::checked(read_f32(bytes, cursor)?).map_err(IoFormatError::Geo)?);
        }
        bricks.insert(coordinate, Brick8x8x8::from_vec(values)?);
    }
    Ok(VdbGrid::from_parts(voxel_size, background, bricks)?)
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

    #[test]
    fn real_world_obj_files_parse() -> Result<(), IoFormatError> {
        let exported = "# Blender\r\nmtllib a.mtl\r\no Cube\r\n\
            v 0 0 0 1 0 0\r\nv 1 0 0 1 0 0\r\nv 1 1 0 0 1 0\r\nv 0 1 0 1.0\r\n\
            vn 0 0 1\r\nvt 0 0\r\nvt 1 0\r\nvt 1 1\r\nvt 0 1\r\n\
            usemtl Material\r\ns off\r\ng group\r\nf 1/1/1 2/2/1 3/3/1 4/4/1\r\n";
        let geo = from_obj(exported)?;
        assert_eq!((geo.len(), geo.prim_count()), (4, 1));
        let uv = geo
            .domain_attribute(AttrDomain::Vertex, "uv")
            .and_then(AttributeColumn::as_vec3)
            .and_then(|column| column.get(2));
        assert_eq!(uv.map(|p| (p.x, p.y, p.z)), Some((1.0, 1.0, 0.0)));
        // Non-ASCII text in a comment or name is not an error.
        assert_eq!(
            from_obj("# café\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n")?.prim_count(),
            1
        );
        Ok(())
    }

    #[test]
    fn negative_obj_indices_count_back_from_the_latest_element() -> Result<(), IoFormatError> {
        let relative = from_obj("v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\nv 5 5 5\nl -1 -2\n")?;
        assert_eq!(relative.vertex_point(0), Some(0));
        assert_eq!(relative.vertex_point(2), Some(2));
        // The polyline's -1 is the fourth point, because it was defined before that line.
        assert_eq!(relative.vertex_point(3), Some(3));
        assert_eq!(relative.vertex_point(4), Some(2));
        for bad in [
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n",
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nf -4 -2 -1\n",
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 9\n",
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nf 1/9 2/9 3/9\n",
        ] {
            assert!(from_obj(bad).is_err(), "{bad:?}");
        }
        Ok(())
    }

    #[test]
    fn unsupported_obj_content_fails_instead_of_vanishing() {
        for bad in [
            "cstype bspline\n",
            "surf 0 1 0 1 1 2\n",
            "v 0 0\n",
            "v 0 0 0 1 2\n",
            "vt\n",
            "v nan 0 0\n",
            "f 1 2\n",
        ] {
            assert!(from_obj(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            from_obj_with_limit("v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\n", 2).err(),
            Some(IoFormatError::ElementLimitExceeded)
        );
    }

    #[test]
    fn uvs_survive_an_obj_round_trip_and_partial_uvs_are_dropped() -> Result<(), IoFormatError> {
        let v = |x, y, z| Vec3::checked(x, y, z).map_err(|_| GeoError::NonFinite);
        let geo = GeoDetail::points(vec![v(0., 0., 0.)?, v(1., 0., 0.)?, v(0., 1., 0.)?])
            .with_primitives(
                vec![PrimRun {
                    kind: PrimKind::Polygon,
                    arity: 3,
                    prim_count: 1,
                }],
                vec![0, 1, 2],
            )?
            .with_domain_attribute(
                AttrDomain::Vertex,
                "uv",
                AttributeColumn::vec3_values(vec![
                    v(0.25, 0.5, 0.)?,
                    v(0.75, 0.5, 0.)?,
                    v(0.5, 1.0, 0.)?,
                ]),
            )?;
        let text = to_obj(&geo);
        assert!(text.contains("f 1/1 2/2 3/3"), "{text}");
        let back = from_obj(&text)?;
        assert_eq!(back.hash(), geo.hash());
        let partial = from_obj("v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nf 1/1 2 3\n")?;
        assert!(partial.domain_attribute(AttrDomain::Vertex, "uv").is_none());
        Ok(())
    }

    #[test]
    fn tetrahedra_round_trip_and_obj_writes_no_faces_for_them() -> Result<(), IoFormatError> {
        let corners = [
            (0.0, 0.0, 0.0),
            (1.0, 0.0, 0.0),
            (0.0, 1.0, 0.0),
            (0.0, 0.0, 1.0),
        ];
        let solid = GeoDetail::points(corners.map(|(x, y, z)| Vec3 { x, y, z }).to_vec())
            .with_primitives(
                vec![PrimRun {
                    kind: PrimKind::Tetrahedron,
                    arity: 4,
                    prim_count: 1,
                }],
                vec![0, 1, 2, 3],
            )?;
        let back = from_ofxg_bytes(&to_ofxg_bytes(&solid))?;
        assert_eq!(back.hash(), solid.hash());
        assert_eq!(back.tet_count(), 1);
        // OBJ cannot hold them, so writing one leaves the points and no faces.
        let text = to_obj(&solid);
        assert!(
            !text.contains(
                "
f "
            ) && !text.contains(
                "
l "
            ),
            "{text}"
        );
        Ok(())
    }

    #[test]
    fn packed_geometry_round_trips_and_nesting_in_files_is_bounded() -> Result<(), IoFormatError> {
        let prototype = Arc::new(from_obj("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n")?);
        let once = GeoDetail::packed_whole(prototype)?;
        let bytes = to_ofxg_bytes(&once);
        let back = from_ofxg_bytes(&bytes)?;
        assert_eq!(back.hash(), once.hash());
        assert_eq!(back.packed_count(), 1);
        assert_eq!(back.packed_prototypes().len(), 1);
        // Files of geometry with no instances never gained a section.
        let plain = from_obj("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n")?;
        let plain_bytes = to_ofxg_bytes(&plain);
        assert_eq!(from_ofxg_bytes(&plain_bytes)?.hash(), plain.hash());

        // A nested file at the depth limit must stop rather than recurse further.
        assert!(read_ofxg(&bytes, DEFAULT_MAX_IO_ELEMENTS, MAX_PACKED_DEPTH).is_err());
        assert!(read_ofxg(&plain_bytes, DEFAULT_MAX_IO_ELEMENTS, MAX_PACKED_DEPTH).is_ok());
        assert!(read_ofxg(&plain_bytes, DEFAULT_MAX_IO_ELEMENTS, MAX_PACKED_DEPTH + 1).is_err());
        // Corruption anywhere in a nested file is detected by some checksum.
        for at in (40..bytes.len()).step_by(7) {
            let mut bad = bytes.clone();
            bad[at] ^= 0xFF;
            assert!(from_ofxg_bytes(&bad).is_err(), "byte {at}");
        }
        Ok(())
    }

    fn sample_grid() -> Result<VdbGrid, GeoError> {
        VdbGrid::new(0.5, 2.0)?
            .with_voxel(1, 1, 1, -0.5)?
            .with_voxel(-20, 3, 40, -0.25)
    }

    #[test]
    fn volumes_round_trip_exactly() -> Result<(), IoFormatError> {
        let grid = sample_grid()?;
        let geo = GeoDetail::from_volumes(vec![Arc::new(grid.clone()), Arc::new(grid)])?;
        let back = from_ofxg_bytes(&to_ofxg_bytes(&geo))?;
        assert_eq!(back.hash(), geo.hash());
        assert_eq!(back.volume_count(), 2);
        assert_eq!(back.bounds(), geo.bounds());
        let voxel = back.volume(1).map(|g| g.get(-20, 3, 40));
        assert_eq!(voxel, Some(-0.25));
        // OBJ cannot hold them, so writing one leaves the anchors and no primitives.
        let text = to_obj(&geo);
        assert_eq!(text.lines().filter(|l| l.starts_with("v ")).count(), 2);
        assert!(
            !text
                .lines()
                .any(|l| l.starts_with("f ") || l.starts_with("l "))
        );
        Ok(())
    }

    /// A grid's bytes, written by hand so that they can say what the writer never would.
    fn grid_bytes(voxel_size: f32, background: f32, bricks: &[((i32, i32, i32), f32)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_f32(&mut bytes, voxel_size);
        write_f32(&mut bytes, background);
        write_u64(&mut bytes, bricks.len() as u64);
        for &((x, y, z), fill) in bricks {
            for coordinate in [x, y, z] {
                bytes.extend_from_slice(&coordinate.to_le_bytes());
            }
            for _ in 0..BRICK_LEN {
                write_f32(&mut bytes, fill);
            }
        }
        bytes
    }

    fn read_grid(bytes: &[u8]) -> Result<VdbGrid, IoFormatError> {
        let mut cursor = 0;
        let grid = read_volume_grid(bytes, &mut cursor, 1 << 20)?;
        assert_eq!(cursor, bytes.len());
        Ok(grid)
    }

    #[test]
    fn a_hostile_grid_is_refused() -> Result<(), IoFormatError> {
        let origin = (0, 0, 0);
        let good = grid_bytes(0.5, 2.0, &[(origin, -1.0), ((0, 0, 1), -1.0)]);
        assert_eq!(read_grid(&good)?.active_brick_count(), 2);
        let refused = |bytes: &[u8]| assert!(read_grid(bytes).is_err(), "{}", bytes.len());
        // Settings that no grid could have been made with.
        refused(&grid_bytes(0.0, 2.0, &[]));
        refused(&grid_bytes(-1.0, 2.0, &[]));
        refused(&grid_bytes(0.5, 0.0, &[]));
        refused(&grid_bytes(f32::NAN, 2.0, &[]));
        refused(&grid_bytes(0.5, f32::INFINITY, &[]));
        refused(&grid_bytes(0.5, 2.0, &[(origin, f32::NAN)]));
        // Two spellings of one grid would hash differently, so order and uniqueness are required.
        refused(&grid_bytes(0.5, 2.0, &[((0, 0, 1), -1.0), (origin, -1.0)]));
        refused(&grid_bytes(0.5, 2.0, &[(origin, -1.0), (origin, -1.0)]));
        // Coordinates at the edge are accepted, and just beyond it would overflow a voxel index.
        let edge = MAX_BRICK_COORDINATE;
        assert!(read_grid(&grid_bytes(0.5, 2.0, &[((edge, 0, 0), -1.0)])).is_ok());
        refused(&grid_bytes(0.5, 2.0, &[((edge + 1, 0, 0), -1.0)]));
        refused(&grid_bytes(0.5, 2.0, &[((0, -edge - 1, 0), -1.0)]));
        refused(&grid_bytes(0.5, 2.0, &[((0, 0, i32::MIN), -1.0)]));
        // Truncated, and claiming more bricks than the file holds.
        refused(&good[..good.len() - 1]);
        let mut lying = grid_bytes(0.5, 2.0, &[]);
        lying.splice(8..16, u64::MAX.to_le_bytes());
        refused(&lying);
        Ok(())
    }
}
