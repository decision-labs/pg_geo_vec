// PostGIS integration module for pg_geo_vec
// Provides bbox extraction from PostGIS geometries using GSERIALIZED format parsing

use pgrx::pg_sys;


/// Extract bounding box from a PostGIS geometry datum
///
/// This function parses the GSERIALIZED format used by PostGIS internally.
/// GSERIALIZED format: gflags (1) + srid (3) + [bbox] + geometry_data
///
/// If the geometry has a cached bbox, we extract it directly.
/// Otherwise, we parse the geometry data to compute the bbox.
///
/// # Arguments
/// * `geom_datum` - A PostgreSQL Datum containing a PostGIS geometry
///
/// # Returns
/// * `Some(BBox2D)` - The 2D bounding box of the geometry
/// * `None` - If the geometry is null, empty, or invalid
#[inline(always)]
pub unsafe fn postgis_extract_bbox(geom_datum: pg_sys::Datum) -> Option<super::BBox2D> {
    // Check for null datum
    if geom_datum.is_null() {
        return None;
    }

    // Get pointer to geometry data
    // PostgreSQL may have TOASTed the data, so we need to detoast it first
    let detoasted = pg_sys::pg_detoast_datum_packed(geom_datum.cast_mut_ptr());

    // Get varlena size and data pointer
    let varlena_ptr = detoasted as *const pg_sys::varlena;

    // Check if it's a short varlena (1-byte header) or regular (4-byte header)
    let header_byte = *(varlena_ptr as *const u8);
    let (data_ptr, varsize) = if (header_byte & 0x01) != 0 {
        // Short varlena: 1-byte header, size in upper 7 bits
        let size = ((header_byte >> 1) & 0x7F) as usize;
        let data = (varlena_ptr as *const u8).add(1);
        (data, size.saturating_sub(1))
    } else {
        // Regular varlena: 4-byte header
        let size_bytes = std::slice::from_raw_parts(varlena_ptr as *const u8, 4);
        let size = u32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]]) >> 2;
        let data = (varlena_ptr as *const u8).add(4);
        (data, (size as usize).saturating_sub(4))
    };

    if varsize < 4 {
        return None;
    }

    // Parse GSERIALIZED format
    parse_gserialized_bbox(data_ptr, varsize)
}

/// Parse GSERIALIZED2 format to extract bounding box
///
/// GSERIALIZED2 format (PostGIS 3.x):
/// - Bytes 0-2: SRID (big-endian, 21 bits significant)
/// - Byte 3: gflags (Z, M, BBOX, GEODETIC, EXTENDED, reserved, VER)
/// - If BBOX flag set: 16+ bytes of cached bbox
/// - Geometry data
unsafe fn parse_gserialized_bbox(ptr: *const u8, len: usize) -> Option<super::BBox2D> {
    if len < 4 {
        return None;
    }

    // gflags is at byte 3
    let gflags = *ptr.add(3);

    // Extract flags
    const G2FLAG_Z: u8 = 0x01;
    const G2FLAG_M: u8 = 0x02;
    const G2FLAG_BBOX: u8 = 0x04;
    const G2FLAG_EXTENDED: u8 = 0x10;

    let has_z = (gflags & G2FLAG_Z) != 0;
    let has_m = (gflags & G2FLAG_M) != 0;
    let has_bbox = (gflags & G2FLAG_BBOX) != 0;
    let has_extended = (gflags & G2FLAG_EXTENDED) != 0;

    // After the 4-byte header, optionally there's an extended flags section
    let mut offset = 4;
    if has_extended {
        offset += 8; // Extended flags take 8 bytes
    }

    // If there's a cached bbox, extract it directly
    if has_bbox {
        // 2D bbox: xmin, xmax, ymin, ymax (4 x float32)
        // 3D bbox adds: zmin, zmax (2 x float32)
        // 4D bbox adds: mmin, mmax (2 x float32)
        let bbox_size = 16 + if has_z { 8 } else { 0 } + if has_m { 8 } else { 0 };

        if offset + bbox_size > len {
            return None;
        }

        // Read the cached bbox (xmin, xmax, ymin, ymax as float32)
        let xmin = read_f32_le(ptr.add(offset));
        let xmax = read_f32_le(ptr.add(offset + 4));
        let ymin = read_f32_le(ptr.add(offset + 8));
        let ymax = read_f32_le(ptr.add(offset + 12));

        return Some(super::BBox2D {
            xmin,
            ymin,
            xmax,
            ymax,
        });
    }

    // No cached bbox - need to parse geometry data
    let geom_ptr = ptr.add(offset);
    let geom_len = len - offset;

    // Parse the serialized geometry to compute bbox
    parse_serialized_geometry_bbox(geom_ptr, geom_len, has_z, has_m)
}

/// Parse serialized geometry data to compute bounding box
///
/// The geometry data format depends on the geometry type.
/// First 4 bytes indicate the type.
unsafe fn parse_serialized_geometry_bbox(
    ptr: *const u8,
    len: usize,
    has_z: bool,
    has_m: bool,
) -> Option<super::BBox2D> {
    if len < 4 {
        return None;
    }

    // First 4 bytes are the geometry type
    let geom_type = read_u32_le(ptr);

    // Calculate coordinate size
    let coords_per_point = 2 + if has_z { 1 } else { 0 } + if has_m { 1 } else { 0 };
    let coord_bytes = coords_per_point * 8;

    let data_ptr = ptr.add(4);
    let data_len = len - 4;

    let mut bbox = super::BBox2D::empty();

    match geom_type {
        1 => {
            // Point
            if data_len >= coord_bytes {
                let x = read_f64_le(data_ptr);
                let y = read_f64_le(data_ptr.add(8));
                bbox.expand(x as f32, y as f32);
            }
        }
        2 => {
            // LineString: npoints + coordinates
            parse_serialized_linestring(&mut bbox, data_ptr, data_len, coord_bytes);
        }
        3 => {
            // Polygon: nrings + [npoints + coordinates]*
            parse_serialized_polygon(&mut bbox, data_ptr, data_len, coord_bytes);
        }
        4 => {
            // MultiPoint: ngeoms + [point]*
            parse_serialized_multi_point(&mut bbox, data_ptr, data_len, coord_bytes);
        }
        5 => {
            // MultiLineString: ngeoms + [linestring]*
            parse_serialized_multi_linestring(&mut bbox, data_ptr, data_len, coord_bytes);
        }
        6 => {
            // MultiPolygon: ngeoms + [polygon]*
            parse_serialized_multi_polygon(&mut bbox, data_ptr, data_len, coord_bytes);
        }
        7 => {
            // GeometryCollection: ngeoms + [geometry]*
            // For simplicity, skip geometry collections
            return None;
        }
        _ => {
            return None;
        }
    }

    if bbox.is_empty() {
        None
    } else {
        Some(bbox)
    }
}

/// Read a little-endian f32 from a pointer
#[inline(always)]
unsafe fn read_f32_le(ptr: *const u8) -> f32 {
    let bytes = std::slice::from_raw_parts(ptr, 4);
    f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Read a little-endian u32 from a pointer
#[inline(always)]
unsafe fn read_u32_le(ptr: *const u8) -> u32 {
    let bytes = std::slice::from_raw_parts(ptr, 4);
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Read a little-endian f64 from a pointer
#[inline(always)]
unsafe fn read_f64_le(ptr: *const u8) -> f64 {
    let bytes = std::slice::from_raw_parts(ptr, 8);
    f64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

/// Parse serialized LineString: npoints + coordinates
unsafe fn parse_serialized_linestring(
    bbox: &mut super::BBox2D,
    ptr: *const u8,
    len: usize,
    coord_bytes: usize,
) {
    if len < 4 {
        return;
    }
    let num_points = read_u32_le(ptr) as usize;
    let mut offset = 4;

    for _ in 0..num_points {
        if offset + coord_bytes > len {
            break;
        }
        let x = read_f64_le(ptr.add(offset));
        let y = read_f64_le(ptr.add(offset + 8));
        bbox.expand(x as f32, y as f32);
        offset += coord_bytes;
    }
}

/// Parse serialized Polygon: nrings + [npoints + coordinates]*
unsafe fn parse_serialized_polygon(
    bbox: &mut super::BBox2D,
    ptr: *const u8,
    len: usize,
    coord_bytes: usize,
) {
    if len < 4 {
        return;
    }
    let num_rings = read_u32_le(ptr) as usize;
    let mut offset = 4;

    for _ in 0..num_rings {
        if offset + 4 > len {
            break;
        }
        let num_points = read_u32_le(ptr.add(offset)) as usize;
        offset += 4;

        for _ in 0..num_points {
            if offset + coord_bytes > len {
                break;
            }
            let x = read_f64_le(ptr.add(offset));
            let y = read_f64_le(ptr.add(offset + 8));
            bbox.expand(x as f32, y as f32);
            offset += coord_bytes;
        }
    }
}

/// Parse serialized MultiPoint: ngeoms + [point coordinates]*
unsafe fn parse_serialized_multi_point(
    bbox: &mut super::BBox2D,
    ptr: *const u8,
    len: usize,
    coord_bytes: usize,
) {
    if len < 4 {
        return;
    }
    let num_points = read_u32_le(ptr) as usize;
    let mut offset = 4;

    for _ in 0..num_points {
        if offset + coord_bytes > len {
            break;
        }
        let x = read_f64_le(ptr.add(offset));
        let y = read_f64_le(ptr.add(offset + 8));
        bbox.expand(x as f32, y as f32);
        offset += coord_bytes;
    }
}

/// Parse serialized MultiLineString: ngeoms + [linestring]*
unsafe fn parse_serialized_multi_linestring(
    bbox: &mut super::BBox2D,
    ptr: *const u8,
    len: usize,
    coord_bytes: usize,
) {
    if len < 4 {
        return;
    }
    let num_geoms = read_u32_le(ptr) as usize;
    let mut offset = 4;

    for _ in 0..num_geoms {
        if offset + 4 > len {
            break;
        }
        let num_points = read_u32_le(ptr.add(offset)) as usize;
        offset += 4;

        for _ in 0..num_points {
            if offset + coord_bytes > len {
                break;
            }
            let x = read_f64_le(ptr.add(offset));
            let y = read_f64_le(ptr.add(offset + 8));
            bbox.expand(x as f32, y as f32);
            offset += coord_bytes;
        }
    }
}

/// Parse serialized MultiPolygon: ngeoms + [polygon]*
unsafe fn parse_serialized_multi_polygon(
    bbox: &mut super::BBox2D,
    ptr: *const u8,
    len: usize,
    coord_bytes: usize,
) {
    if len < 4 {
        return;
    }
    let num_geoms = read_u32_le(ptr) as usize;
    let mut offset = 4;

    for _ in 0..num_geoms {
        // Each polygon: nrings + [npoints + coordinates]*
        if offset + 4 > len {
            break;
        }
        let num_rings = read_u32_le(ptr.add(offset)) as usize;
        offset += 4;

        for _ in 0..num_rings {
            if offset + 4 > len {
                break;
            }
            let num_points = read_u32_le(ptr.add(offset)) as usize;
            offset += 4;

            for _ in 0..num_points {
                if offset + coord_bytes > len {
                    break;
                }
                let x = read_f64_le(ptr.add(offset));
                let y = read_f64_le(ptr.add(offset + 8));
                bbox.expand(x as f32, y as f32);
                offset += coord_bytes;
            }
        }
    }
}
