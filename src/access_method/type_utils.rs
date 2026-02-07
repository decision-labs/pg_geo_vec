use pgrx::{pg_sys, PgRelation};

/// True when the zero-based index attribute position is PostGIS geometry type.
/// Uses format_type_be to look up the type name from the catalog by OID,
/// avoiding TypenameGetTypid which can fail during index builds.
pub fn is_geometry_column(index: &PgRelation, attr_idx: usize) -> bool {
    let tuple_desc = index.tuple_desc();
    let Some(attr) = tuple_desc.get(attr_idx) else {
        return false;
    };
    let oid = attr.type_oid().value();
    if oid == pg_sys::InvalidOid {
        return false;
    }
    // format_type_be may return schema-qualified name like "public.geometry"
    let type_name = unsafe {
        let cstr = pg_sys::format_type_be(oid);
        std::ffi::CStr::from_ptr(cstr)
            .to_str()
            .unwrap_or("")
            .to_string()
    };
    // Match "geometry" or "*.geometry" (schema-qualified)
    type_name == "geometry"
        || type_name
            .rsplit_once('.')
            .map_or(false, |(_, name)| name == "geometry")
}
