//! JAGR-2: Content-Addressable Storage
//!
//! pnpm-style CAS with hard links for maximum disk efficiency and fast installs.
//! Single copy of each package version globally, shared across all projects.

mod cas;
mod hardlink;
mod integrity;

pub use cas::{CASConfig, ContentAddressableStore, StoreEntry};
pub use hardlink::{link_package, LinkResult, LinkType};
pub use integrity::{compute_integrity, verify_integrity, IntegrityHash};
