// PostGIS integration module for pg_geo_vec
//
// Extracts 2D bounding boxes from PostGIS geometry datums by reading the
// cached bbox directly from the GSERIALIZED header. No link-time or dlsym
// dependency on PostGIS — we only need the geometry's binary layout.
//
// If no cached bbox is present in the geometry, falls back to calling
// PostGIS's LWGEOM_to_BOX2DF via DirectFunctionCall (which forces PostGIS
// to compute the bbox).

use std::ffi::CString;

use libc::{c_char, c_void};
use pgrx::pg_sys;

type LwgeomToBox2df = unsafe extern "C-unwind" fn(pg_sys::FunctionCallInfo) -> pg_sys::Datum;

// GSERIALIZED v1 gflags (PostGIS 2.x compatible)
const G1FLAG_BBOX: u8 = 0x04; // bit 2 (Z=0x01, M=0x02, BBOX=0x04, GEODETIC=0x08)

// GSERIALIZED v2 gflags (PostGIS 3.2+)
const G2FLAG_BBOX: u8 = 0x04; // bit 2
const G2FLAG_EXTENDED: u8 = 0x10; // bit 4
const G2FLAG_VER_0: u8 = 0x20; // bit 5 — indicates GSERIALIZED v2

/// Force-load the PostGIS shared library into the current backend process.
///
/// PostgreSQL loads extension shared libraries lazily — only when a function from
/// that library is first called. During index build, our code runs before any
/// PostGIS function has been called in this backend, so PostGIS symbols aren't
/// available via dlsym. This function forces the load by looking up PostGIS's
/// library path from pg_proc and calling load_file().
///
/// Safe to call multiple times; only loads once.
pub fn ensure_postgis_loaded() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static LOADED: AtomicBool = AtomicBool::new(false);
    if LOADED.load(Ordering::Relaxed) {
        return;
    }

    // Already loaded? Check if a known PostGIS symbol resolves.
    let sym = CString::new("LWGEOM_to_BOX2DF").unwrap();
    let ptr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, sym.as_ptr() as *const c_char) };
    if !ptr.is_null() {
        LOADED.store(true, Ordering::Relaxed);
        return;
    }

    // Look up PostGIS library path from pg_proc (e.g. "$libdir/postgis-3")
    // and force-load it via PostgreSQL's load_file().
    unsafe {
        use pgrx::pg_sys::*;

        let query = CString::new(
            "SELECT probin FROM pg_proc WHERE proname = 'postgis_version' LIMIT 1",
        )
        .unwrap();

        let spi_ret = SPI_connect();
        if spi_ret != SPI_OK_CONNECT as i32 {
            LOADED.store(true, Ordering::Relaxed);
            return;
        }

        let ret = SPI_execute(query.as_ptr(), true, 1);
        if ret == SPI_OK_SELECT as i32 && SPI_processed > 0 && !SPI_tuptable.is_null() {
            let tuptable = &*SPI_tuptable;
            let tuple = *tuptable.vals;
            let tupdesc = tuptable.tupdesc;
            let cstr = SPI_getvalue(tuple, tupdesc, 1);
            if !cstr.is_null() {
                // load_file handles $libdir expansion and RTLD_GLOBAL
                load_file(cstr, false);
            }
        }

        SPI_finish();
    }

    LOADED.store(true, Ordering::Relaxed);
}

/// Resolve SQL-callable PostGIS function symbol `LWGEOM_to_BOX2DF` as fallback.
fn resolve_lwgeom_to_box2df() -> Option<LwgeomToBox2df> {
    use std::sync::atomic::{AtomicPtr, Ordering};
    static SYMBOL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

    let cached = SYMBOL.load(Ordering::Relaxed);
    if !cached.is_null() {
        return Some(unsafe { std::mem::transmute::<*mut c_void, LwgeomToBox2df>(cached) });
    }

    let symbol_name = CString::new("LWGEOM_to_BOX2DF").ok()?;
    let ptr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, symbol_name.as_ptr() as *const c_char) };
    if ptr.is_null() {
        return None;
    }
    SYMBOL.store(ptr, Ordering::Relaxed);
    Some(unsafe { std::mem::transmute::<*mut c_void, LwgeomToBox2df>(ptr) })
}

/// Returns true if PostGIS bbox extraction is available.
pub fn postgis_bbox_api_available() -> bool {
    true
}

/// Ensure PostGIS bbox API is available (loads PostGIS if needed for fallback).
pub fn ensure_postgis_bbox_api() {
    ensure_postgis_loaded();
}

/// Extract 2D bbox from a PostGIS geometry datum.
///
/// Primary path: read the cached bbox directly from the GSERIALIZED header.
/// Fallback: call LWGEOM_to_BOX2DF via dlsym if no cached bbox.
///
/// Returns `None` if the datum is null or bbox extraction fails.
#[inline(always)]
pub unsafe fn postgis_extract_bbox(geom_datum: pg_sys::Datum) -> Option<super::BBox2D> {
    if geom_datum.is_null() {
        return None;
    }

    let varlena_ptr = geom_datum.cast_mut_ptr::<pg_sys::varlena>();
    if varlena_ptr.is_null() {
        return None;
    }

    // Detoast if necessary.
    let detoasted = pg_sys::pg_detoast_datum(varlena_ptr);
    if detoasted.is_null() {
        return None;
    }

    let result = extract_bbox_from_gserialized(detoasted as *const u8, geom_datum);

    // Free detoasted copy if it was allocated.
    if detoasted != varlena_ptr {
        pg_sys::pfree(detoasted as *mut c_void);
    }

    result
}

/// Read the bbox from a GSERIALIZED geometry in memory.
///
/// GSERIALIZED layout (both v1 and v2):
///   varlena_header (4 bytes, or 1 byte for short varlena)
///   srid[3]        (3 bytes — SRID)
///   gflags         (1 byte — flags, interpretation differs by version)
///   [ext_flags]    (1 byte — only in v2 with EXTENDED flag)
///   [bbox]         (4 × f32 for 2D — xmin, xmax, ymin, ymax)
///   geometry_data  (variable)
unsafe fn extract_bbox_from_gserialized(
    raw: *const u8,
    geom_datum: pg_sys::Datum,
) -> Option<super::BBox2D> {
    // Skip varlena header to get GSERIALIZED payload.
    let payload = vardata(raw);

    // GSERIALIZED: srid[0..3] then gflags at offset 3
    let gflags = *payload.add(3);

    // Detect GSERIALIZED version and check has_bbox flag.
    let is_v2 = gflags & G2FLAG_VER_0 != 0;
    let has_bbox = if is_v2 {
        gflags & G2FLAG_BBOX != 0
    } else {
        gflags & G1FLAG_BBOX != 0
    };

    if !has_bbox {
        // No cached bbox in header — fall back to PostGIS function call.
        return postgis_extract_bbox_fallback(geom_datum);
    }

    // Bbox starts after srid(3) + gflags(1) [+ ext_flags(1) for v2 extended].
    let bbox_offset: usize = if is_v2 && (gflags & G2FLAG_EXTENDED != 0) {
        3 + 1 + 1 // srid + gflags + ext_flags
    } else {
        3 + 1 // srid + gflags
    };

    let bbox_ptr = payload.add(bbox_offset) as *const f32;
    Some(super::BBox2D {
        xmin: *bbox_ptr,
        xmax: *bbox_ptr.add(1),
        ymin: *bbox_ptr.add(2),
        ymax: *bbox_ptr.add(3),
    })
}

/// Fallback: call LWGEOM_to_BOX2DF for geometries without a cached bbox.
unsafe fn postgis_extract_bbox_fallback(geom_datum: pg_sys::Datum) -> Option<super::BBox2D> {
    let lwgeom_to_box2df = resolve_lwgeom_to_box2df()?;
    let box2df_datum =
        pg_sys::DirectFunctionCall1Coll(Some(lwgeom_to_box2df), pg_sys::InvalidOid, geom_datum);
    if box2df_datum.is_null() {
        return None;
    }

    // BOX2DF is a fixed-length 16-byte struct: 4 × float32
    let ptr = box2df_datum.cast_mut_ptr::<f32>();
    if ptr.is_null() {
        return None;
    }
    Some(super::BBox2D {
        xmin: *ptr,
        xmax: *ptr.add(1),
        ymin: *ptr.add(2),
        ymax: *ptr.add(3),
    })
}

/// Get pointer to varlena data (after the varlena header).
#[inline(always)]
unsafe fn vardata(ptr: *const u8) -> *const u8 {
    let first_byte = *ptr;
    if first_byte & 0x01 != 0 {
        // Short varlena: 1-byte header
        ptr.add(1)
    } else {
        // Regular 4-byte header
        ptr.add(4)
    }
}
