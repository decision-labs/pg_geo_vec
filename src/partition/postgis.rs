// PostGIS integration module for pg_geo_vec
// Uses canonical PostGIS bbox extraction helper APIs.

use std::ffi::CString;
use std::sync::OnceLock;

use libc::{c_char, c_int, c_void};
use pgrx::pg_sys;

const LW_SUCCESS: c_int = 1;

type GserializedDatumGetGboxP = unsafe extern "C" fn(pg_sys::Datum, *mut GBox) -> c_int;

/// PostGIS GBOX layout (liblwgeom.h): flags + xmin/xmax/ymin/ymax/zmin/zmax/mmin/mmax
#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct GBox {
    flags: u16,
    xmin: f64,
    xmax: f64,
    ymin: f64,
    ymax: f64,
    zmin: f64,
    zmax: f64,
    mmin: f64,
    mmax: f64,
}

/// Resolve `gserialized_datum_get_gbox_p` from symbols already loaded in the backend process.
///
/// This avoids hard link-time dependency on PostGIS while still calling the canonical helper
/// when PostGIS is installed and loaded.
fn resolve_gserialized_datum_get_gbox_p() -> Option<GserializedDatumGetGboxP> {
    static SYMBOL: OnceLock<Option<GserializedDatumGetGboxP>> = OnceLock::new();
    static WARNED_MISSING_SYMBOL: OnceLock<()> = OnceLock::new();

    *SYMBOL.get_or_init(|| {
        let symbol_name = CString::new("gserialized_datum_get_gbox_p").ok()?;
        // SAFETY: RTLD_DEFAULT searches currently loaded symbols in the backend process.
        let ptr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, symbol_name.as_ptr() as *const c_char) };
        if ptr.is_null() {
            let _ = WARNED_MISSING_SYMBOL.get_or_init(|| {
                pgrx::warning!(
                    "PostGIS symbol gserialized_datum_get_gbox_p not found; spatial bbox extraction is disabled"
                );
            });
            return None;
        }

        // SAFETY: Symbol name and function signature are taken from PostGIS headers.
        let f: GserializedDatumGetGboxP =
            unsafe { std::mem::transmute::<*mut c_void, GserializedDatumGetGboxP>(ptr) };
        Some(f)
    })
}

/// Returns true if canonical PostGIS bbox helper API is available in this backend process.
pub fn postgis_bbox_api_available() -> bool {
    resolve_gserialized_datum_get_gbox_p().is_some()
}

/// Ensure canonical PostGIS bbox helper API is available.
/// Raises an ERROR with actionable guidance when unavailable.
pub fn ensure_postgis_bbox_api() {
    if !postgis_bbox_api_available() {
        pgrx::error!(
            "PostGIS bbox helper API is unavailable. Ensure PostGIS is installed and loaded (e.g. CREATE EXTENSION postgis)."
        );
    }
}

/// Extract 2D bbox from a PostGIS geometry datum using PostGIS canonical helper API.
///
/// Returns `None` if PostGIS helper symbol is unavailable, the datum is null, or PostGIS
/// reports extraction failure (e.g. NULL/EMPTY/invalid geometry).
#[inline(always)]
pub unsafe fn postgis_extract_bbox(geom_datum: pg_sys::Datum) -> Option<super::BBox2D> {
    if geom_datum.is_null() {
        return None;
    }

    let get_gbox = resolve_gserialized_datum_get_gbox_p()?;
    let mut gbox = GBox {
        flags: 0,
        xmin: 0.0,
        xmax: 0.0,
        ymin: 0.0,
        ymax: 0.0,
        zmin: 0.0,
        zmax: 0.0,
        mmin: 0.0,
        mmax: 0.0,
    };

    let status = get_gbox(geom_datum, &mut gbox as *mut GBox);
    if status != LW_SUCCESS {
        return None;
    }

    Some(super::BBox2D {
        xmin: gbox.xmin as f32,
        xmax: gbox.xmax as f32,
        ymin: gbox.ymin as f32,
        ymax: gbox.ymax as f32,
    })
}
