mod cloner;
mod installer;
mod store_bridge;

pub use cloner::*;
pub use installer::*;
pub(crate) use store_bridge::{
    emit_route, materialize_from_store_unpacked, publish_l0_from_node_modules,
};

/// True when `name@version` has a complete `store_unpacked` CAS tree.
pub(crate) fn store_bridge_unpacked_ready(name: &str, version: &str) -> bool {
    store_bridge::unpacked_source_for(name, version).is_some()
}
