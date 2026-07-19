//! Lattice Resolution Cache — persistent (package, constraint) → version cache.
pub fn cache_key(name: &str, constraint: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    constraint.hash(&mut h);
    h.finish()
}
