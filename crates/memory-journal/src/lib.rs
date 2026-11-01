//! The project journal, mind maps and flowcharts (IF-7): one SQLite file per project (CON-22),
//! with entries, flags, links and lexical search (CON-21). The daemon that owns these files is a
//! later slice; `plugin` serves the same store in-process behind the `Memory` interface.

pub mod journal;
pub mod mindmap;
pub mod mindmap_flow;
pub mod mindmap_svg;

use std::sync::Once;

static SQLITE_VEC_INIT: Once = Once::new();

/// Registers the vector extension the journal's schema needs, once per process.
pub fn ensure_sqlite_vec_registered() {
    SQLITE_VEC_INIT.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is the C entry point the sqlite-vec crate exports; this
        // transmute to the auto-extension signature is that crate's documented usage.
        unsafe {
            #[allow(clippy::missing_transmute_annotations)]
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ())));
        }
    });
}
