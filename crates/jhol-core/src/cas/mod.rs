//! JAGR-2: Content-Addressable Storage
//! 
//! pnpm-style CAS with hard links for maximum disk efficiency and fast installs.
//! Single copy of each package version globally, shared across all projects.

mod cas;
mod hardlink;
mod integrity;

pub use cas::{ContentAddressableStore, StoreEntry, CASConfig};
pub use hardlink::{LinkType, link_package, LinkResult};
pub use integrity::{IntegrityHash, verify_integrity, compute_integrity};
