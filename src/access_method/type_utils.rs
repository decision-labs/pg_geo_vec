use std::ffi::CString;
use std::sync::OnceLock;

use pgrx::{pg_sys, PgRelation};

/// Cached OID for PostGIS geometry type. Returns InvalidOid when type is unavailable.
pub fn geometry_type_oid() -> pg_sys::Oid {
    static GEOMETRY_OID: OnceLock<pg_sys::Oid> = OnceLock::new();
    *GEOMETRY_OID.get_or_init(|| {
        let type_name = CString::new("geometry").expect("CString::new(\"geometry\") must succeed");
        // SAFETY: TypenameGetTypid is pure catalog lookup in current backend.
        unsafe { pg_sys::TypenameGetTypid(type_name.as_ptr()) }
    })
}

/// True when the zero-based index attribute position is PostGIS geometry type.
pub fn is_geometry_column(index: &PgRelation, attr_idx: usize) -> bool {
    let tuple_desc = index.tuple_desc();
    let Some(attr) = tuple_desc.get(attr_idx) else {
        return false;
    };
    let geom_oid = geometry_type_oid();
    geom_oid != pg_sys::InvalidOid && attr.type_oid().value() == geom_oid
}
