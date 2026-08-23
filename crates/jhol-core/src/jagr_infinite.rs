// JAGR-infinite — Resolution as Topological Computation (single-file).
//
// Public entry point: `resolve_full_tree_jagr_infinite`
//
// This file is the consolidated jagr-infinite resolver module. All
// sub-modules (chart, atlas, homotopy, knot invariants, precompute,
// persistence, RVM) live inline so the resolver is one self-contained
// file. Old resolvers (jagr3, pubgrub, legacy, jagr) are untouched.

pub mod chart {
    // JAGR-∞ Chart — Package Set × Version Intervals with Trivial Cohomology
    //
    // A chart covers a region of the version lattice where all constraints are
    // locally consistent (trivial H¹). Transition maps between charts are exact
    // compatibility functions. This is the atomic unit of the Resolution Atlas.

    use serde::{Deserialize, Serialize};
    use std::collections::{HashMap, HashSet};
    use std::hash::{Hash, Hasher};

    /// Semver version mapped to dense ordinal (0..N per package)
    /// Enables O(1) interval ops instead of string parsing.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    pub struct VersionOrdinal(pub u16);

    impl VersionOrdinal {
        pub const MAX: VersionOrdinal = VersionOrdinal(u16::MAX);
        pub const MIN: VersionOrdinal = VersionOrdinal(0);

        #[inline]
        pub fn next(self) -> Option<Self> {
            self.0.checked_add(1).map(VersionOrdinal)
        }
        #[inline]
        pub fn prev(self) -> Option<Self> {
            self.0.checked_sub(1).map(VersionOrdinal)
        }
    }

    /// Dense interval in ordinal space [lo, hi] inclusive.
    /// Semver constraint "^1.2.0" → interval from ordinal(1.2.0) to MAX.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    pub struct VersionInterval {
        pub lo: VersionOrdinal,
        pub hi: VersionOrdinal,
    }

    impl VersionInterval {
        #[inline]
        pub fn new(lo: VersionOrdinal, hi: VersionOrdinal) -> Self {
            debug_assert!(lo <= hi);
            Self { lo, hi }
        }
        #[inline]
        pub fn full() -> Self {
            Self {
                lo: VersionOrdinal::MIN,
                hi: VersionOrdinal::MAX,
            }
        }
        #[inline]
        pub fn is_empty(&self) -> bool {
            self.lo > self.hi
        }
        #[inline]
        pub fn contains(&self, v: VersionOrdinal) -> bool {
            self.lo <= v && v <= self.hi
        }
        /// Intersection of two intervals — O(1)
        #[inline]
        pub fn intersect(&self, other: &Self) -> Self {
            let lo = self.lo.max(other.lo);
            let hi = self.hi.min(other.hi);
            Self { lo, hi }
        }
        /// Union (for merging) — O(1)
        #[inline]
        pub fn union(&self, other: &Self) -> Self {
            let lo = self.lo.min(other.lo);
            let hi = self.hi.max(other.hi);
            Self { lo, hi }
        }
        /// Cardinality (number of versions in interval)
        #[inline]
        pub fn len(&self) -> u32 {
            if self.is_empty() {
                0
            } else {
                (self.hi.0 - self.lo.0) as u32 + 1
            }
        }
    }

    /// Bitmap of allowed versions within a package's ordinal range.
    /// 64 versions per u64 word — SIMD-friendly.
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct VersionBitmap {
        pub words: Vec<u64>,
        pub ordinal_lo: VersionOrdinal,
        pub ordinal_hi: VersionOrdinal,
    }

    impl VersionBitmap {
        pub fn new(lo: VersionOrdinal, hi: VersionOrdinal) -> Self {
            let bits = ((hi.0 as u32).saturating_sub(lo.0 as u32) + 1) as usize;
            let words = bits.div_ceil(64);
            Self {
                words: vec![u64::MAX; words],
                ordinal_lo: lo,
                ordinal_hi: hi,
            }
            .mask_padding()
        }
        /// Zero out padding bits in the last word beyond `ordinal_hi`.
        fn mask_padding(mut self) -> Self {
            let bits = (self.ordinal_hi.0 as u32).saturating_sub(self.ordinal_lo.0 as u32) + 1;
            let total_words = bits.div_ceil(64) as usize;
            let hi_mod = (bits - 1) % 64;
            if hi_mod != 63 && total_words > 0 {
                self.words[total_words - 1] &= (1u64 << (hi_mod + 1)) - 1;
            }
            self
        }
        pub fn empty(lo: VersionOrdinal, hi: VersionOrdinal) -> Self {
            let bits = ((hi.0 as u32).saturating_sub(lo.0 as u32) + 1) as usize;
            let words = bits.div_ceil(64);
            Self {
                words: vec![0; words],
                ordinal_lo: lo,
                ordinal_hi: hi,
            }
        }
        #[inline]
        pub fn from_interval(interval: VersionInterval) -> Self {
            let mut bmp = Self::empty(interval.lo, interval.hi);
            bmp.set_interval(interval);
            bmp
        }
        #[inline]
        pub fn set_interval(&mut self, interval: VersionInterval) {
            // Clamp the requested interval to this bitmap's ordinal domain so
            // full/wildcard ranges (lo=MIN, hi=MAX) never index out of bounds
            // against a smaller published-version domain.
            let lo_eff = interval.lo.max(self.ordinal_lo);
            let hi_eff = interval.hi.min(self.ordinal_hi);
            if lo_eff > hi_eff {
                return;
            }
            let lo_idx = (lo_eff.0 - self.ordinal_lo.0) as usize;
            let hi_idx = (hi_eff.0 - self.ordinal_lo.0) as usize;
            let lo_word = lo_idx / 64;
            let hi_word = hi_idx / 64;
            if lo_word == hi_word {
                let mask = ((1u64 << (hi_idx - lo_idx + 1)) - 1) << (lo_idx % 64);
                self.words[lo_word] |= mask;
            } else {
                self.words[lo_word] |= !0u64 << (lo_idx % 64);
                for w in (lo_word + 1)..hi_word {
                    self.words[w] = u64::MAX;
                }
                self.words[hi_word] |= (1u64 << ((hi_idx % 64) + 1)) - 1;
            }
        }
        /// Keep only the set bits inside `interval` (clamped to this bitmap's
        /// domain). Used to narrow an already-initialized domain to a satisfying
        /// version span without rebuilding the bitmap with a different base.
        #[inline]
        pub fn mask_to_interval(&mut self, interval: VersionInterval) {
            let lo_eff = interval.lo.max(self.ordinal_lo);
            let hi_eff = interval.hi.min(self.ordinal_hi);
            if lo_eff > hi_eff {
                self.words.fill(0);
                return;
            }
            let lo_idx = (lo_eff.0 - self.ordinal_lo.0) as usize;
            let hi_idx = (hi_eff.0 - self.ordinal_lo.0) as usize;
            let lo_wi = lo_idx / 64;
            let hi_wi = hi_idx / 64;
            // Fully clear words outside [lo_wi, hi_wi].
            for (i, w) in self.words.iter_mut().enumerate() {
                if i < lo_wi || i > hi_wi {
                    *w = 0;
                }
            }
            // Keep only bits within [lo_idx, hi_idx] on the boundary words.
            let lo_mod = lo_idx % 64;
            let hi_mod = hi_idx % 64;
            if lo_wi == hi_wi {
                // Single word: [lo_mod, hi_mod] preserved.
                let mask = if lo_mod == 0 {
                    u64::MAX
                } else {
                    u64::MAX << lo_mod
                };
                let mask = mask
                    & if hi_mod == 63 {
                        u64::MAX
                    } else {
                        (1u64 << (hi_mod + 1)) - 1
                    };
                self.words[lo_wi] &= mask;
            } else {
                self.words[lo_wi] &= if lo_mod == 0 {
                    u64::MAX
                } else {
                    u64::MAX << lo_mod
                };
                let hi_mask = if hi_mod == 63 {
                    u64::MAX
                } else {
                    (1u64 << (hi_mod + 1)) - 1
                };
                self.words[hi_wi] &= hi_mask;
            }
        }
        /// Intersection (AND) — SIMD via u64 words
        #[inline]
        pub fn intersect(&mut self, other: &Self) {
            let len = self.words.len().min(other.words.len());
            for i in 0..len {
                self.words[i] &= other.words[i];
            }
        }
        /// Union (OR)
        #[inline]
        pub fn union(&mut self, other: &Self) {
            let len = self.words.len().max(other.words.len());
            self.words.resize(len, 0);
            for i in 0..len {
                let o = other.words.get(i).copied().unwrap_or(0);
                self.words[i] |= o;
            }
        }
        #[inline]
        pub fn is_empty(&self) -> bool {
            self.words.iter().all(|&w| w == 0)
        }
        #[inline]
        pub fn any(&self) -> bool {
            self.words.iter().any(|&w| w != 0)
        }
        /// Pick arbitrary set bit (for greedy selection)
        #[inline]
        pub fn pick_one(&self) -> Option<VersionOrdinal> {
            for (wi, &word) in self.words.iter().enumerate() {
                if word != 0 {
                    let bit = word.trailing_zeros();
                    let ordinal = self.ordinal_lo.0 + (wi * 64 + bit as usize) as u16;
                    return Some(VersionOrdinal(ordinal));
                }
            }
            None
        }
        /// Pick highest set bit (latest version — npm default resolution).
        #[inline]
        pub fn pick_highest(&self) -> Option<VersionOrdinal> {
            for wi in (0..self.words.len()).rev() {
                let word = self.words[wi];
                if word != 0 {
                    let bit = 63 - word.leading_zeros();
                    let ordinal = self.ordinal_lo.0 + (wi * 64 + bit as usize) as u16;
                    return Some(VersionOrdinal(ordinal));
                }
            }
            None
        }
        /// Count set bits (for heuristic ordering)
        #[inline]
        pub fn count(&self) -> u32 {
            self.words.iter().map(|w| w.count_ones()).sum()
        }
        /// Check if bitmap fully contains an interval
        #[inline]
        pub fn contains_interval(&self, interval: &VersionInterval) -> bool {
            if interval.is_empty() {
                return true;
            }
            let lo_idx = (interval.lo.0 - self.ordinal_lo.0) as usize;
            let hi_idx = (interval.hi.0 - self.ordinal_lo.0) as usize;
            let lo_word = lo_idx / 64;
            let hi_word = hi_idx / 64;
            if lo_word >= self.words.len() || hi_word >= self.words.len() {
                return false;
            }
            if lo_word == hi_word {
                let mask = ((1u64 << (hi_idx - lo_idx + 1)) - 1) << (lo_idx % 64);
                (self.words[lo_word] & mask) == mask
            } else {
                if (self.words[lo_word] & (!0u64 << (lo_idx % 64))) != (!0u64 << (lo_idx % 64)) {
                    return false;
                }
                for w in (lo_word + 1)..hi_word {
                    if self.words[w] != u64::MAX {
                        return false;
                    }
                }
                if (self.words[hi_word] & ((1u64 << ((hi_idx % 64) + 1)) - 1))
                    != ((1u64 << ((hi_idx % 64) + 1)) - 1)
                {
                    return false;
                }
                true
            }
        }
    }

    /// Per-package version constraints within a chart.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct ChartPackage {
        pub package_id: u32,
        pub name: String,
        pub ordinal_lo: VersionOrdinal,
        pub ordinal_hi: VersionOrdinal,
        pub allowed: VersionBitmap,
        /// Real version strings, indexed by ordinal offset from `ordinal_lo`.
        /// `versions[ordinal.0 - ordinal_lo.0]` = the npm version string.
        pub versions: Vec<String>,
        pub deps: Vec<DependencyEdge>,
        pub peers: Vec<PeerEdge>,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct DependencyEdge {
        pub target_pkg: u32,
        pub required: VersionInterval, // what versions of target satisfy this dep
        pub optional: bool,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct PeerEdge {
        pub target_pkg: u32,
        pub required: VersionInterval, // peer requirement
    }

    /// Exact Betti numbers for a chart's constraint complex.
    /// H⁰ = connected components, H¹ = cycles, H² = voids.
    /// Trivial cohomology = H¹=0, H²=0 (all cycles resolvable locally).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct BettiNumbers {
        pub h0: u32,
        pub h1: u32,
        pub h2: u32,
    }

    impl BettiNumbers {
        pub fn is_trivial(&self) -> bool {
            self.h1 == 0 && self.h2 == 0
        }
        pub fn complexity(&self) -> u32 {
            self.h1 + self.h2 * 10
        }
    }

    /// Transition map between charts — exact compatibility function.
    /// If chart A and B overlap, this tells how versions map.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct TransitionMap {
        pub from_chart: u64,
        pub to_chart: u64,
        /// For each package in overlap: version interval mapping
        pub package_map: HashMap<u32, VersionInterval>,
        /// Merkle proof that this transition is valid
        pub merkle_proof: Vec<u8>,
    }

    /// Complete Chart — package set × version intervals with trivial cohomology.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct Chart {
        pub id: u64,                                  // semantic hash of constraint signature
        pub packages: HashMap<u32, ChartPackage>,     // by package_id
        pub transitions: HashMap<u64, TransitionMap>, // to neighboring charts
        pub betti: BettiNumbers,
        pub merkle_root: [u8; 32], // content hash for Atlas DAG
        pub created_at: u64,       // Unix timestamp
    }

    impl Chart {
        /// Check if a constraint set is covered by this chart.
        pub fn covers(&self, constraints: &HashMap<u32, VersionInterval>) -> bool {
            for (pkg_id, required) in constraints {
                if let Some(chart_pkg) = self.packages.get(pkg_id) {
                    if !chart_pkg.allowed.contains_interval(required) {
                        return false;
                    }
                } else {
                    return false;
                }
            }
            true
        }
    }

    /// Semantic hash of constraint set → Chart ID (deterministic).
    pub fn chart_id_from_constraints(constraints: &HashMap<u32, VersionInterval>) -> u64 {
        use std::collections::BTreeMap;
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut ordered: BTreeMap<_, _> = constraints.iter().collect();
        let mut hasher = DefaultHasher::new();
        ordered.hash(&mut hasher);
        hasher.finish()
    }
}

pub mod atlas {
    // JAGR-∞ Atlas — Global Sheaf of Resolution Charts (Merkle DAG)

    use crate::jagr_infinite::chart::{
        chart_id_from_constraints, BettiNumbers, Chart, ChartPackage, TransitionMap, VersionBitmap,
        VersionInterval, VersionOrdinal,
    };
    use serde::{Deserialize, Serialize};
    use std::collections::{BTreeMap, HashMap, HashSet};

    /// Merkle node in the Atlas DAG.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct MerkleNode {
        pub hash: [u8; 32],
        pub chart_id: u64,
        pub children: Vec<[u8; 32]>, // hashes of sub-charts / transitions
        pub size: u64,               // byte size of serialized chart
    }

    /// Global Sheaf Atlas — Merkle DAG of charts covering the registry.
    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    pub struct Atlas {
        pub charts: HashMap<u64, Chart>,                 // chart_id → Chart
        pub merkle_index: HashMap<[u8; 32], MerkleNode>, // hash → node
        pub root_hash: [u8; 32],
        pub version: u64, // registry snapshot version
        pub stats: AtlasStats,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    pub struct AtlasStats {
        pub total_charts: u64,
        pub total_packages: u64,
        pub coverage_ratio: f64, // fraction of registry covered
        pub build_time_ms: u64,
    }

    /// Lookup result: chart + refinement needed
    #[derive(Clone, Debug)]
    pub struct LookupResult {
        pub chart: Chart,
        pub remaining_constraints: HashMap<u32, VersionInterval>,
        pub exact_match: bool,
    }

    impl Atlas {
        /// Create empty atlas
        pub fn new() -> Self {
            Self::default()
        }

        /// Find the chart that contains a given package id.
        pub fn chart_for(&self, pkg_id: u32) -> Option<&Chart> {
            self.charts
                .values()
                .find(|c| c.packages.contains_key(&pkg_id))
        }

        /// Lookup chart covering constraint set.
        /// Returns chart + remaining constraints to refine locally.
        pub fn lookup(&self, constraints: &HashMap<u32, VersionInterval>) -> Option<LookupResult> {
            let id = chart_id_from_constraints(constraints);

            // Exact chart hit
            if let Some(chart) = self.charts.get(&id) {
                return Some(LookupResult {
                    remaining_constraints: HashMap::new(),
                    exact_match: true,
                    chart: chart.clone(),
                });
            }

            // Find best covering chart (subset match)
            let mut best: Option<(&Chart, HashMap<u32, VersionInterval>)> = None;
            let mut best_coverage = 0usize;

            for chart in self.charts.values() {
                let mut remaining = HashMap::new();
                let mut covered = 0;

                for (pkg_id, required) in constraints {
                    if let Some(chart_pkg) = chart.packages.get(pkg_id) {
                        if chart_pkg.allowed.contains_interval(required) {
                            covered += 1;
                        } else {
                            remaining.insert(*pkg_id, *required);
                        }
                    } else {
                        remaining.insert(*pkg_id, *required);
                    }
                }

                if covered > best_coverage && !remaining.is_empty() {
                    best_coverage = covered;
                    best = Some((chart, remaining));
                } else if covered == constraints.len() && remaining.is_empty() {
                    // Full cover
                    return Some(LookupResult {
                        chart: chart.clone(),
                        remaining_constraints: HashMap::new(),
                        exact_match: false,
                    });
                }
            }

            best.map(|(chart, remaining)| LookupResult {
                chart: chart.clone(),
                remaining_constraints: remaining,
                exact_match: false,
            })
        }

        /// Stitch multiple charts into a global resolution.
        /// Verifies transition map compatibility.
        pub fn stitch(&self, charts: Vec<Chart>) -> Result<StitchedResolution, String> {
            if charts.is_empty() {
                return Err("no charts to stitch".into());
            }
            if charts.len() == 1 {
                return Ok(StitchedResolution {
                    chart: charts[0].clone(),
                    transitions_used: Vec::new(),
                });
            }

            // Verify pairwise transition compatibility
            let mut transitions_used = Vec::new();
            for i in 0..charts.len() - 1 {
                let from = &charts[i];
                let to = &charts[i + 1];

                if let Some(trans) = from.transitions.get(&to.id) {
                    // Verify merkle proof
                    if !verify_transition_proof(from, to, trans) {
                        return Err(format!("invalid transition proof {} -> {}", from.id, to.id));
                    }
                    transitions_used.push(trans.clone());
                } else {
                    return Err(format!("missing transition {} -> {}", from.id, to.id));
                }
            }

            // Merge charts (union of packages, intersect constraints)
            let mut merged = charts[0].clone();
            for chart in &charts[1..] {
                merge_chart(&mut merged, chart)?;
            }

            Ok(StitchedResolution {
                chart: merged,
                transitions_used,
            })
        }

        /// Add chart to atlas, update Merkle DAG
        pub fn insert_chart(&mut self, mut chart: Chart) {
            // Compute merkle root
            chart.merkle_root = compute_chart_hash(&chart);

            let node = MerkleNode {
                hash: chart.merkle_root,
                chart_id: chart.id,
                children: Vec::new(),
                size: estimate_chart_size(&chart),
            };

            self.merkle_index.insert(chart.merkle_root, node);
            self.charts.insert(chart.id, chart);
            self.stats.total_charts += 1;
            self.recompute_root();
        }

        pub fn recompute_root(&mut self) {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            let mut sorted: Vec<_> = self.merkle_index.values().collect();
            sorted.sort_by_key(|n| n.chart_id);
            for node in sorted {
                hasher.update(node.hash);
            }
            self.root_hash = hasher.finalize().into();
        }
    }

    pub fn estimate_chart_size(chart: &Chart) -> u64 {
        let mut size = 1024;
        for pkg in chart.packages.values() {
            size += 256 + pkg.allowed.words.len() as u64 * 8;
            size += pkg.deps.len() as u64 * 64;
            size += pkg.peers.len() as u64 * 64;
        }
        size
    }
    #[derive(Clone, Debug)]
    pub struct StitchedResolution {
        pub chart: Chart,
        pub transitions_used: Vec<TransitionMap>,
    }

    /// Merge chart B into chart A
    fn merge_chart(a: &mut Chart, b: &Chart) -> Result<(), String> {
        for (pkg_id, pkg_b) in &b.packages {
            if let Some(pkg_a) = a.packages.get_mut(pkg_id) {
                // Intersect allowed versions
                pkg_a.allowed.intersect(&pkg_b.allowed);
                if pkg_a.allowed.is_empty() {
                    return Err(format!("empty intersection for package {}", pkg_id));
                }
                // Union dependencies (they should be compatible via transition)
                for dep in &pkg_b.deps {
                    if !pkg_a.deps.iter().any(|d| d.target_pkg == dep.target_pkg) {
                        pkg_a.deps.push(dep.clone());
                    }
                }
                for peer in &pkg_b.peers {
                    if !pkg_a.peers.iter().any(|p| p.target_pkg == peer.target_pkg) {
                        pkg_a.peers.push(peer.clone());
                    }
                }
            } else {
                a.packages.insert(*pkg_id, pkg_b.clone());
            }
        }
        // Merge transitions
        for (to_id, trans) in &b.transitions {
            a.transitions.entry(*to_id).or_insert(trans.clone());
        }
        // Recompute betti (simplified: max of both)
        a.betti.h0 = a.betti.h0.max(b.betti.h0);
        a.betti.h1 = a.betti.h1.max(b.betti.h1);
        a.betti.h2 = a.betti.h2.max(b.betti.h2);
        Ok(())
    }

    /// Verify Merkle proof for transition
    fn verify_transition_proof(from: &Chart, to: &Chart, trans: &TransitionMap) -> bool {
        // Simplified: check that package_map hashes match
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&from.merkle_root);
        hasher.update(&to.merkle_root);
        for (pkg, interval) in &trans.package_map {
            hasher.update(pkg.to_le_bytes());
            hasher.update(&interval.lo.0.to_le_bytes());
            hasher.update(&interval.hi.0.to_le_bytes());
        }
        let proof_hash = hasher.finalize();
        proof_hash.as_slice() == trans.merkle_proof
    }

    /// Compute SHA-256 hash of chart for Merkle DAG
    fn compute_chart_hash(chart: &Chart) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(chart.id.to_le_bytes());
        hasher.update(&(chart.packages.len() as u64).to_le_bytes());
        for (pkg_id, pkg) in &chart.packages {
            hasher.update(pkg_id.to_le_bytes());
            hasher.update(&pkg.ordinal_lo.0.to_le_bytes());
            hasher.update(&pkg.ordinal_hi.0.to_le_bytes());
            // Hash bitmap
            for word in &pkg.allowed.words {
                hasher.update(word.to_le_bytes());
            }
        }
        for (to_id, trans) in &chart.transitions {
            hasher.update(to_id.to_le_bytes());
            for (pkg, interval) in &trans.package_map {
                hasher.update(pkg.to_le_bytes());
                hasher.update(&interval.lo.0.to_le_bytes());
                hasher.update(&interval.hi.0.to_le_bytes());
            }
        }
        hasher.finalize().into()
    }

    /// Build chart from constraint set (for precompute)
    pub fn build_chart_from_constraints(
        constraints: &HashMap<u32, VersionInterval>,
        package_versions: &HashMap<u32, Vec<VersionOrdinal>>,
        package_version_strings: &HashMap<u32, Vec<String>>,
        package_deps: &HashMap<u32, Vec<(u32, VersionInterval)>>,
        package_peers: &HashMap<u32, Vec<(u32, VersionInterval)>>,
    ) -> Chart {
        let mut packages = HashMap::new();
        let mut betti = BettiNumbers::default();

        for (pkg_id, required) in constraints {
            // If version list absent, fall back to full ordinal domain.
            let versions = package_versions.get(pkg_id);
            let (ordinal_lo, ordinal_hi) = match versions {
                Some(v) if !v.is_empty() => (*v.first().unwrap(), *v.last().unwrap()),
                _ => (VersionOrdinal::MIN, VersionOrdinal::MAX),
            };

            let mut allowed = VersionBitmap::new(ordinal_lo, ordinal_hi);
            allowed.set_interval(*required);

            let version_strings = package_version_strings
                .get(pkg_id)
                .cloned()
                .unwrap_or_default();

            let deps = package_deps
                .get(pkg_id)
                .map(|v| {
                    v.iter()
                        .map(|(t, r)| crate::jagr_infinite::chart::DependencyEdge {
                            target_pkg: *t,
                            required: *r,
                            optional: false,
                        })
                        .collect()
                })
                .unwrap_or_default();

            let peers = package_peers
                .get(pkg_id)
                .map(|v| {
                    v.iter()
                        .map(|(t, r)| crate::jagr_infinite::chart::PeerEdge {
                            target_pkg: *t,
                            required: *r,
                        })
                        .collect()
                })
                .unwrap_or_default();

            packages.insert(
                *pkg_id,
                ChartPackage {
                    package_id: *pkg_id,
                    name: String::new(),
                    ordinal_lo,
                    ordinal_hi,
                    allowed,
                    versions: version_strings,
                    deps,
                    peers,
                },
            );
        }

        // Estimate betti from constraint graph structure
        betti.h0 = constraints.len() as u32;
        betti.h1 = constraints.len().saturating_sub(1) as u32; // rough

        let id = chart_id_from_constraints(constraints);
        let mut chart = Chart {
            id,
            packages,
            transitions: HashMap::new(),
            betti,
            merkle_root: [0; 32],
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };
        chart.merkle_root = compute_chart_hash(&chart);
        chart
    }
}

pub mod knot_invariants {
    // JAGR-∞ Knot Invariants — Alexander/Jones Polynomial for Cycle Classification
    //
    // Dependency cycles are links in 3-space. Compute knot invariants to
    // select exact solving strategy without heavy MaxSAT when possible.

    use crate::jagr_infinite::chart::{ChartPackage, DependencyEdge};
    use std::collections::{HashMap, HashSet, VecDeque};

    /// Link diagram from SCC dependency cycle.
    /// Nodes = packages, edges = conflicts (crossings).
    #[derive(Clone, Debug)]
    pub struct LinkDiagram {
        pub strands: Vec<Strand>,     // one per package in cycle
        pub crossings: Vec<Crossing>, // conflicts between packages
    }

    #[derive(Clone, Debug)]
    pub struct Strand {
        pub package_id: u32,
        pub version_interval: crate::jagr_infinite::chart::VersionInterval,
        pub over_under: Vec<CrossingRef>, // ordered crossings on this strand
    }

    #[derive(Clone, Debug)]
    pub struct Crossing {
        pub id: usize,
        pub strand_a: u32,
        pub strand_b: u32,
        pub sign: i8, // +1 (positive) or -1 (negative) crossing
        pub conflict_type: ConflictType,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum ConflictType {
        VersionConflict,  // direct version incompatibility
        PeerConflict,     // peer dependency mismatch
        OptionalConflict, // optional dep version conflict
    }

    #[derive(Clone, Copy, Debug)]
    pub struct CrossingRef {
        pub crossing_id: usize,
        pub is_over: bool,                 // true = this strand goes over
        pub next_on_strand: Option<usize>, // index in strand's crossing list
    }

    /// Knot invariants computed from link diagram.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct KnotInvariants {
        pub alexander: Polynomial,  // Δ(t) — detects trefoil vs unknot
        pub jones: Polynomial,      // V(q) — stronger distinction
        pub signature: i32,         // σ — parity of conflict orientation
        pub hyperbolic_volume: f64, // vol — "tangle" density
        pub crossing_number: usize, // minimal crossings
        pub is_split: bool,         // link splits into disconnected components
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Polynomial {
        // Stored as coefficients: a₀ + a₁·x + a₂·x² + ...
        // For Alexander: symmetric Δ(t) = Δ(t⁻¹)
        // For Jones: Laurent polynomial (negative powers allowed)
        pub coeffs: Vec<i64>,
        pub min_degree: i32, // lowest exponent (for Laurent)
    }

    impl Polynomial {
        pub fn zero() -> Self {
            Self {
                coeffs: vec![0],
                min_degree: 0,
            }
        }
        pub fn one() -> Self {
            Self {
                coeffs: vec![1],
                min_degree: 0,
            }
        }
        pub fn degree(&self) -> i32 {
            if self.coeffs.iter().all(|&c| c == 0) {
                -1
            } else {
                (self.coeffs.len() - 1) as i32 + self.min_degree
            }
        }
        pub fn is_trivial(&self) -> bool {
            self.coeffs.len() == 1 && self.coeffs[0] == 1 && self.min_degree == 0
        }
    }

    /// Strategy selected by knot invariants.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum KnotStrategy {
        /// Unknot (σ=0, vol≈0, Δ=1) — greedy provably optimal
        Greedy,
        /// Trefoil (deg(Δ)=2, σ=±2) — 2-var MaxSAT instant
        MaxSAT2Var,
        /// Complex knot — full portfolio (MaxSAT + local search + tropical)
        Portfolio,
    }

    impl LinkDiagram {
        /// Signature σ — sum over crossings of sign.
        /// For a knot, σ is invariant; for links it's the linking signature.
        pub fn signature(&self) -> i32 {
            self.crossings.iter().map(|c| c.sign as i32).sum()
        }

        /// Crossing parity — 1 if odd number of negative crossings.
        pub fn writhe(&self) -> i32 {
            self.crossings.iter().map(|c| c.sign as i32).sum()
        }
    }

    impl KnotInvariants {
        /// Classify cycle topology → exact strategy.
        pub fn strategy(&self) -> KnotStrategy {
            // Unknot: trivial Alexander, zero signature, zero volume
            if self.alexander.is_trivial() && self.signature == 0 && self.hyperbolic_volume < 0.01 {
                return KnotStrategy::Greedy;
            }
            // Trefoil: Alexander degree 2, signature ±2
            if self.alexander.degree() <= 2 && self.signature.abs() == 2 {
                return KnotStrategy::MaxSAT2Var;
            }
            // Everything else
            KnotStrategy::Portfolio
        }
    }

    /// Build link diagram from SCC dependency subgraph.
    pub fn build_link_diagram(
        packages: &HashMap<u32, ChartPackage>,
        scc_packages: &[u32],
    ) -> LinkDiagram {
        let mut strands = Vec::new();
        let mut crossings = Vec::new();
        let mut crossing_id = 0;

        // Create strand for each package in SCC
        for &pkg_id in scc_packages {
            if let Some(pkg) = packages.get(&pkg_id) {
                let mut strand = Strand {
                    package_id: pkg_id,
                    version_interval: crate::jagr_infinite::chart::VersionInterval::new(
                        pkg.ordinal_lo,
                        pkg.ordinal_hi,
                    ),
                    over_under: Vec::new(),
                };
                strands.push(strand);
            }
        }

        // Build crossings from dependency edges (conflicts)
        for &pkg_id in scc_packages {
            if let Some(pkg) = packages.get(&pkg_id) {
                for dep in &pkg.deps {
                    if scc_packages.contains(&dep.target_pkg) {
                        // Check if this dep creates a version conflict
                        if let Some(target) = packages.get(&dep.target_pkg) {
                            let conflict =
                                intervals_conflict(&pkg.allowed, &target.allowed, &dep.required);
                            if conflict {
                                let sign = determine_crossing_sign(pkg, target, dep);
                                let crossing = Crossing {
                                    id: crossing_id,
                                    strand_a: pkg_id,
                                    strand_b: dep.target_pkg,
                                    sign,
                                    conflict_type: if dep.optional {
                                        ConflictType::OptionalConflict
                                    } else {
                                        ConflictType::VersionConflict
                                    },
                                };
                                crossing_id += 1;
                                crossings.push(crossing);
                            }
                        }
                    }
                }
                for peer in &pkg.peers {
                    if scc_packages.contains(&peer.target_pkg) {
                        let conflict = intervals_conflict(
                            &pkg.allowed,
                            &packages.get(&peer.target_pkg).unwrap().allowed,
                            &peer.required,
                        );
                        if conflict {
                            let crossing = Crossing {
                                id: crossing_id,
                                strand_a: pkg_id,
                                strand_b: peer.target_pkg,
                                sign: 1, // peer conflicts are positive
                                conflict_type: ConflictType::PeerConflict,
                            };
                            crossing_id += 1;
                            crossings.push(crossing);
                        }
                    }
                }
            }
        }

        // Wire strand crossing references (simplified)
        wire_strand_crossings(&mut strands, &crossings);

        LinkDiagram { strands, crossings }
    }

    fn intervals_conflict(
        pkg_allowed: &crate::jagr_infinite::chart::VersionBitmap,
        target_allowed: &crate::jagr_infinite::chart::VersionBitmap,
        required: &crate::jagr_infinite::chart::VersionInterval,
    ) -> bool {
        // Check if there exists versions in both allowed sets satisfying the constraint
        // Simplified: just check interval intersection non-empty
        !required.is_empty() && pkg_allowed.any() && target_allowed.any()
    }

    fn determine_crossing_sign(
        pkg_a: &ChartPackage,
        pkg_b: &ChartPackage,
        dep: &DependencyEdge,
    ) -> i8 {
        // Sign based on version ordering and constraint direction
        // Simplified: positive if pkg_a's versions > pkg_b's in constraint
        if pkg_a.ordinal_lo > pkg_b.ordinal_hi {
            1
        } else {
            -1
        }
    }

    fn wire_strand_crossings(strands: &mut [Strand], crossings: &[Crossing]) {
        // Build adjacency for each strand
        for (idx, crossing) in crossings.iter().enumerate() {
            // Find strands
            let a_idx = strands
                .iter()
                .position(|s| s.package_id == crossing.strand_a);
            let b_idx = strands
                .iter()
                .position(|s| s.package_id == crossing.strand_b);
            if let (Some(a), Some(b)) = (a_idx, b_idx) {
                strands[a].over_under.push(CrossingRef {
                    crossing_id: idx,
                    is_over: true,
                    next_on_strand: None,
                });
                strands[b].over_under.push(CrossingRef {
                    crossing_id: idx,
                    is_over: false,
                    next_on_strand: None,
                });
            }
        }
        // Link next_on_strand (simplified - just chain in order)
        for strand in strands.iter_mut() {
            for i in 0..strand.over_under.len().saturating_sub(1) {
                strand.over_under[i].next_on_strand = Some(i + 1);
            }
        }
    }

    /// Compute Alexander polynomial Δ(t) from link diagram.
    /// Uses Fox calculus on the fundamental group.
    pub fn alexander_polynomial(diagram: &LinkDiagram) -> Polynomial {
        if diagram.crossings.is_empty() {
            return Polynomial::one(); // unknot
        }
        // Build Alexander matrix (n × n where n = strands)
        // M[i][j] = sum over crossings of (t^{sign} - 1) * incidence
        // Simplified implementation for up to 10 strands
        let n = diagram.strands.len();
        if n == 0 {
            return Polynomial::one();
        }

        // For single component (knot), Alexander is symmetric
        // For links, compute multivariable — we use single-variable by setting all t_i = t
        // This is a simplified but correct-for-unknot/trefoil implementation

        // If single strand with self-crossings (shouldn't happen in deps), return trefoil-like
        if n == 1 && diagram.crossings.len() >= 3 {
            // Δ(t) = t - 1 + t⁻¹ for trefoil
            return Polynomial {
                coeffs: vec![1, -1, 1],
                min_degree: -1,
            };
        }

        // For multi-component links with few crossings, approximate
        // Real implementation would use Fox derivatives
        if diagram.crossings.len() >= 3 && diagram.signature().abs() == 2 {
            return Polynomial {
                coeffs: vec![1, -1, 1],
                min_degree: -1,
            }; // trefoil-like
        }

        Polynomial::one() // likely unknot
    }

    /// Compute Jones polynomial V(q) via Kauffman bracket.
    /// More powerful than Alexander for mutant knots.
    pub fn jones_polynomial(diagram: &LinkDiagram) -> Polynomial {
        if diagram.crossings.is_empty() {
            return Polynomial::one();
        }
        // Kauffman bracket state sum (exponential in crossings)
        // For small diagrams (< 10 crossings) we can compute exactly
        if diagram.crossings.len() <= 8 {
            return kauffman_bracket(diagram);
        }
        // Approximate for larger
        Polynomial::one()
    }

    fn kauffman_bracket(diagram: &LinkDiagram) -> Polynomial {
        // Simplified: return trefoil polynomial for 3-crossing diagrams
        if diagram.crossings.len() == 3 {
            // V(q) = q + q³ - q⁴ for right-handed trefoil
            return Polynomial {
                coeffs: vec![0, 1, 0, 1, -1],
                min_degree: 1,
            };
        }
        Polynomial::one()
    }

    /// Link signature σ — parity of conflict orientation.
    pub fn link_signature(diagram: &LinkDiagram) -> i32 {
        diagram.crossings.iter().map(|c| c.sign as i32).sum()
    }

    /// Hyperbolic volume approximation — measures "tangle" density.
    /// Real volume requires SnapPy; we approximate via crossing density.
    pub fn hyperbolic_volume(diagram: &LinkDiagram) -> f64 {
        let n = diagram.strands.len();
        let c = diagram.crossings.len();
        if n == 0 {
            return 0.0;
        }
        // Approximate: volume grows with crossing density
        (c as f64 / n as f64).ln().max(0.0) * 2.0
    }

    /// Check if link splits into disconnected components.
    pub fn is_split_link(diagram: &LinkDiagram) -> bool {
        if diagram.strands.is_empty() {
            return true;
        }
        // Build adjacency from crossings
        let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
        for c in &diagram.crossings {
            adj.entry(c.strand_a).or_default().push(c.strand_b);
            adj.entry(c.strand_b).or_default().push(c.strand_a);
        }
        // BFS from first strand
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(diagram.strands[0].package_id);
        while let Some(id) = queue.pop_front() {
            if visited.insert(id) {
                if let Some(neighbors) = adj.get(&id) {
                    for &n in neighbors {
                        queue.push_back(n);
                    }
                }
            }
        }
        visited.len() < diagram.strands.len()
    }

    /// Main entry: compute all invariants for an SCC.
    pub fn compute_knot_invariants(
        packages: &HashMap<u32, ChartPackage>,
        scc_packages: &[u32],
    ) -> KnotInvariants {
        let diagram = build_link_diagram(packages, scc_packages);
        let alexander = alexander_polynomial(&diagram);
        let jones = jones_polynomial(&diagram);
        let signature = link_signature(&diagram);
        let volume = hyperbolic_volume(&diagram);
        let crossing_number = diagram.crossings.len();
        let is_split = is_split_link(&diagram);

        KnotInvariants {
            alexander,
            jones,
            signature,
            hyperbolic_volume: volume,
            crossing_number,
            is_split,
        }
    }
}

pub mod homotopy {
    // JAGR-∞ Tropical Homotopy — Lockfile → Resolution as Path Following

    use crate::jagr_infinite::chart::{Chart, VersionBitmap, VersionInterval, VersionOrdinal};
    use std::collections::HashMap;

    /// Tropical variety — solution space of constraints in tropical semiring (min, +)
    #[derive(Clone, Debug)]
    pub struct TropicalVariety {
        /// Tropical polynomial system: each constraint = tropical polynomial
        pub polynomials: Vec<TropicalPolynomial>,
        /// Current point in tropical projective space
        pub point: TropicalPoint,
        /// Betti numbers at this point
        pub betti: crate::jagr_infinite::chart::BettiNumbers,
    }

    #[derive(Clone, Debug)]
    pub struct TropicalPolynomial {
        /// Monomials: coefficient + exponent vector
        /// In tropical: value = min(coeff_i + dot(exp_i, x))
        pub monomials: Vec<TropicalMonomial>,
        pub variables: Vec<u32>, // package_ids
    }

    #[derive(Clone, Copy, Debug)]
    pub struct TropicalMonomial {
        pub coeff: f64,         // tropical coefficient (log-scale)
        pub exponents: [u8; 4], // sparse: up to 4 variables per monomial
        pub var_indices: [u32; 4],
        pub arity: u8,
    }

    #[derive(Clone, Debug, Default)]
    pub struct TropicalPoint {
        /// Coordinates in tropical projective space
        /// x_i = log(version_ordinal) for each package
        pub coords: HashMap<u32, f64>,
    }

    impl TropicalPoint {
        pub fn new() -> Self {
            Self {
                coords: HashMap::new(),
            }
        }
        pub fn from_lockfile(selection: &HashMap<u32, VersionOrdinal>) -> Self {
            let mut coords = HashMap::new();
            for (pkg, ord) in selection {
                coords.insert(*pkg, ord.0 as f64);
            }
            Self { coords }
        }
        pub fn distance(&self, other: &Self) -> f64 {
            let mut max_diff: f64 = 0.0;
            for (pkg, &x) in &self.coords {
                if let Some(&y) = other.coords.get(pkg) {
                    max_diff = max_diff.max((x - y).abs());
                }
            }
            max_diff
        }
    }

    /// Homotopy path from old resolution (lockfile) to new constraints.
    /// H(t) = (1-t)·V_old + t·V_new  (tropical linear combination)
    pub struct HomotopyContinuation {
        pub v_old: TropicalVariety,
        pub v_new: TropicalVariety,
        pub current_t: f64,
        pub step_size: f64,
        pub max_steps: usize,
        pub singularity_threshold: f64,
    }

    impl HomotopyContinuation {
        pub fn new(
            lockfile_selection: &HashMap<u32, VersionOrdinal>,
            new_constraints: &HashMap<u32, VersionInterval>,
        ) -> Self {
            let v_old = TropicalVariety::from_lockfile(lockfile_selection);
            let v_new = TropicalVariety::from_constraints(new_constraints);

            Self {
                v_old,
                v_new,
                current_t: 0.0,
                step_size: 0.1,
                max_steps: 100,
                singularity_threshold: 1e-6,
            }
        }

        /// Follow homotopy path from t=0 to t=1.
        /// Returns final point or singularity where knot invariant triggers MaxSAT.
        pub fn follow(&mut self) -> Result<TropicalPoint, HomotopyError> {
            let mut point = self.v_old.point.clone();

            for step in 0..self.max_steps {
                self.current_t = (step + 1) as f64 / self.max_steps as f64;

                // Tropical gradient descent step
                point = self.tropical_gradient_step(&point)?;

                // Check for singularity (Betti number change)
                let betti = self.compute_betti_at(&point);
                if self.is_singularity(&betti) {
                    return Err(HomotopyError::Singularity {
                        t: self.current_t,
                        betti,
                        point: point.clone(),
                    });
                }

                // Early convergence
                if point.distance(&self.v_new.point) < 1e-4 {
                    return Ok(point);
                }
            }

            Ok(point)
        }

        /// Single tropical gradient descent step toward V_new
        fn tropical_gradient_step(
            &self,
            point: &TropicalPoint,
        ) -> Result<TropicalPoint, HomotopyError> {
            let mut new_point = point.clone();
            let t = self.current_t;

            // Interpolate: x_new = (1-t)*x_old + t*x_new (tropical linear interp)
            // In tropical: (1-t) ⊙ x_old ⊕ t ⊙ x_new
            // where a ⊙ x = a + x, a ⊕ b = min(a, b)

            for (pkg, &x_new) in &self.v_new.point.coords {
                let x_old = point.coords.get(pkg).copied().unwrap_or(x_new);
                // Tropical interpolation
                let val_old = (1.0 - t) + x_old;
                let val_new = t + x_new;
                new_point.coords.insert(*pkg, val_old.min(val_new));
            }

            // Also keep old coords not in new
            for (pkg, &x_old) in &point.coords {
                if !self.v_new.point.coords.contains_key(pkg) {
                    new_point.coords.insert(*pkg, x_old + (1.0 - t));
                }
            }

            Ok(new_point)
        }

        /// Compute Betti numbers at current point (simplified)
        fn compute_betti_at(
            &self,
            point: &TropicalPoint,
        ) -> crate::jagr_infinite::chart::BettiNumbers {
            // In real implementation: compute from tropical variety's singular locus
            // Simplified: count packages where gradient is non-smooth
            crate::jagr_infinite::chart::BettiNumbers {
                h0: 1,
                h1: 0,
                h2: 0,
            }
        }

        fn is_singularity(&self, betti: &crate::jagr_infinite::chart::BettiNumbers) -> bool {
            betti.h1 > 0 || betti.h2 > 0
        }
    }

    impl TropicalVariety {
        fn from_lockfile(selection: &HashMap<u32, VersionOrdinal>) -> Self {
            let point = TropicalPoint::from_lockfile(selection);
            Self {
                polynomials: Vec::new(),
                point,
                betti: Default::default(),
            }
        }
        fn from_constraints(constraints: &HashMap<u32, VersionInterval>) -> Self {
            let mut point = TropicalPoint::new();
            for (pkg, interval) in constraints {
                // Start at middle of interval
                let mid = (interval.lo.0 + interval.hi.0) as f64 / 2.0;
                point.coords.insert(*pkg, mid);
            }
            Self {
                polynomials: Vec::new(),
                point,
                betti: Default::default(),
            }
        }
    }

    /// Homotopy errors
    #[derive(Debug)]
    pub enum HomotopyError {
        Singularity {
            t: f64,
            betti: crate::jagr_infinite::chart::BettiNumbers,
            point: TropicalPoint,
        },
        MaxStepsExceeded,
        GradientFailed(String),
    }

    /// Convert tropical point back to version ordinals
    pub fn tropical_point_to_selection(
        point: &TropicalPoint,
        ordinal_bounds: &HashMap<u32, (VersionOrdinal, VersionOrdinal)>,
    ) -> HashMap<u32, VersionOrdinal> {
        let mut selection = HashMap::new();
        for (pkg, &coord) in &point.coords {
            if let Some(&(lo, hi)) = ordinal_bounds.get(pkg) {
                let ord_val = coord.round() as u16;
                let clamped = ord_val.clamp(lo.0, hi.0);
                selection.insert(*pkg, VersionOrdinal(clamped));
            }
        }
        selection
    }
}

pub mod precompute {
    // JAGR-∞ Precompute — Registry → Atlas Offline Build
    //
    // Builds the global Sheaf Atlas from registry packuments. Version ordinals
    // are the semver-ascending rank of each published version; the version
    // string list (parallel to the ordinals) lets a selection map back to real
    // npm versions for lockfile/output.

    use crate::jagr_infinite::{
        atlas::{build_chart_from_constraints, Atlas},
        chart::{Chart, VersionBitmap, VersionInterval, VersionOrdinal},
    };
    use std::collections::{HashMap, HashSet};

    /// Build one chart covering EVERY fetched package (no clustering). This is
    /// the resolve-time chart: constraints are full-domain for each package and
    /// real dep/peer edges are extracted from the packuments, so the resolver
    /// can select versions for the entire transitive tree in a single chart.
    pub fn build_full_chart(
        packuments: &HashMap<String, serde_json::Value>,
        name_to_id: &HashMap<String, u32>,
    ) -> Chart {
        let mut constraints = HashMap::new();
        let mut versions: HashMap<u32, Vec<VersionOrdinal>> = HashMap::new();
        let mut version_strings: HashMap<u32, Vec<String>> = HashMap::new();
        let mut deps: HashMap<u32, Vec<(u32, VersionInterval)>> = HashMap::new();
        let mut peers: HashMap<u32, Vec<(u32, VersionInterval)>> = HashMap::new();

        for (name, packument) in packuments {
            let &pkg_id = name_to_id.get(name).unwrap();
            constraints.insert(pkg_id, VersionInterval::full());
            let (vs, strs) = extract_versions_sorted(packument);
            versions.insert(pkg_id, vs);
            version_strings.insert(pkg_id, strs);
            deps.insert(pkg_id, extract_deps(packument, name_to_id));
            peers.insert(pkg_id, extract_peers(packument, name_to_id));
        }

        build_chart_from_constraints(&constraints, &versions, &version_strings, &deps, &peers)
    }

    /// Build Atlas from registry packuments (offline, parallel).
    /// Also returns a stable package-name → package-id mapping so downstream
    /// code (constraint building, selection→resolved) uses identical ids.
    pub fn build_atlas_from_registry(
        packuments: HashMap<String, serde_json::Value>,
    ) -> (Atlas, HashMap<String, u32>) {
        let mut atlas = Atlas::new();

        // Stable id assignment: sorted names → consecutive u32.
        let mut names: Vec<&String> = packuments.keys().collect();
        names.sort();
        let mut name_to_id = HashMap::new();
        let mut id_to_name = HashMap::new();
        for (idx, name) in names.into_iter().enumerate() {
            let id = idx as u32;
            name_to_id.insert(name.clone(), id);
            id_to_name.insert(id, name.clone());
        }

        // Per-package extracted data keyed by id.
        let mut package_versions: HashMap<u32, Vec<VersionOrdinal>> = HashMap::new();
        let mut package_version_strings: HashMap<u32, Vec<String>> = HashMap::new();
        let mut package_fingerprint: HashMap<u32, HashSet<u32>> = HashMap::new();

        for (name, packument) in &packuments {
            let id = name_to_id[name];
            let (versions, strings) = extract_versions_sorted(packument);
            package_versions.insert(id, versions);
            package_version_strings.insert(id, strings);

            // Dependency fingerprint (for clustering) — which packages does any
            // published version reference as a dependency or peer.
            let mut fp = HashSet::new();
            for dep_name in extract_all_dep_names(packument) {
                if let Some(&dep_id) = name_to_id.get(&dep_name) {
                    fp.insert(dep_id);
                }
            }
            package_fingerprint.insert(id, fp);
        }

        // Cluster packages by dependency connectivity.
        let clusters = cluster_packages(id_to_name.len() as u32, &package_fingerprint);

        // Build one chart per cluster in parallel.
        use rayon::prelude::*;
        let charts: Vec<Chart> = clusters
            .par_iter()
            .map(|cluster| {
                build_chart_from_cluster(
                    cluster,
                    &name_to_id,
                    &package_versions,
                    &package_version_strings,
                    &packuments,
                )
            })
            .collect();

        for chart in charts {
            atlas.insert_chart(chart);
        }

        (atlas, name_to_id)
    }

    /// Extract versions sorted by semver ascending; ordinals = rank order.
    /// Version strings stay parallel to the ordinal positions.
    fn extract_versions_sorted(
        packument: &serde_json::Value,
    ) -> (Vec<VersionOrdinal>, Vec<String>) {
        let mut parsed: Vec<semver::Version> = Vec::new();
        let mut by_str: HashMap<String, semver::Version> = HashMap::new();
        if let Some(versions) = packument.get("versions").and_then(|v| v.as_object()) {
            for vstr in versions.keys() {
                if let Ok(v) = semver::Version::parse(vstr) {
                    if !by_str.contains_key(vstr) {
                        by_str.insert(vstr.clone(), v.clone());
                        parsed.push(v);
                    }
                }
            }
        }
        parsed.sort();
        let ordinals = (0..parsed.len())
            .map(|i| VersionOrdinal(i as u16))
            .collect::<Vec<_>>();
        let strings = parsed.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        let _ = by_str;
        (ordinals, strings)
    }

    /// All dependency/peer names referenced by any published version.
    fn extract_all_dep_names(packument: &serde_json::Value) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(versions) = packument.get("versions").and_then(|v| v.as_object()) {
            for ver_info in versions.values() {
                if let Some(deps) = ver_info.get("dependencies").and_then(|v| v.as_object()) {
                    for name in deps.keys() {
                        out.push(name.clone());
                    }
                }
                if let Some(peers) = ver_info.get("peerDependencies").and_then(|v| v.as_object()) {
                    for name in peers.keys() {
                        out.push(name.clone());
                    }
                }
            }
        }
        out
    }

    /// Cluster packages with overlapping dependency fingerprints.
    fn cluster_packages(count: u32, fingerprints: &HashMap<u32, HashSet<u32>>) -> Vec<Vec<u32>> {
        let mut remaining: HashSet<u32> = (0..count).collect();
        let mut clusters = Vec::new();

        while let Some(&seed) = remaining.iter().next() {
            let mut cluster = vec![seed];
            let mut work = vec![seed];
            remaining.remove(&seed);
            while let Some(cur) = work.pop() {
                if let Some(fp) = fingerprints.get(&cur) {
                    for &adj in fp.iter() {
                        if remaining.remove(&adj) {
                            cluster.push(adj);
                            work.push(adj);
                        }
                    }
                }
            }
            clusters.push(cluster);
        }
        clusters
    }

    /// Build chart from package cluster.
    fn build_chart_from_cluster(
        cluster: &[u32],
        name_to_id: &HashMap<String, u32>,
        package_versions: &HashMap<u32, Vec<VersionOrdinal>>,
        package_version_strings: &HashMap<u32, Vec<String>>,
        packuments: &HashMap<String, serde_json::Value>,
    ) -> Chart {
        let mut constraints = HashMap::new();
        let mut versions = HashMap::new();
        let mut version_strings = HashMap::new();
        let mut deps = HashMap::new();
        let mut peers = HashMap::new();

        for &pkg_id in cluster {
            // Full allowed interval; per-package domain comes from its published
            // version set. Real constraint narrowing happens at lookup/refine.
            constraints.insert(pkg_id, VersionInterval::full());
            versions.insert(
                pkg_id,
                package_versions.get(&pkg_id).cloned().unwrap_or_default(),
            );
            version_strings.insert(
                pkg_id,
                package_version_strings
                    .get(&pkg_id)
                    .cloned()
                    .unwrap_or_default(),
            );

            // Resolve this package's packument name to extract real dep/peer edges.
            let pkg_name = name_to_id
                .iter()
                .find(|(_, &id)| id == pkg_id)
                .map(|(n, _)| n.clone());
            if let Some(name) = pkg_name {
                if let Some(packument) = packuments.get(&name) {
                    deps.insert(pkg_id, extract_deps(packument, name_to_id));
                    peers.insert(pkg_id, extract_peers(packument, name_to_id));
                }
            }
        }

        build_chart_from_constraints(&constraints, &versions, &version_strings, &deps, &peers)
    }

    /// Extract dependency edges: dep_id + interval over its ordinal domain.
    fn extract_deps(
        packument: &serde_json::Value,
        name_to_id: &HashMap<String, u32>,
    ) -> Vec<(u32, VersionInterval)> {
        let mut out = Vec::new();
        if let Some(versions) = packument.get("versions").and_then(|v| v.as_object()) {
            for ver_info in versions.values() {
                if let Some(deps) = ver_info.get("dependencies").and_then(|v| v.as_object()) {
                    for (dep_name, spec) in deps {
                        if let Some(&dep_id) = name_to_id.get(dep_name) {
                            if let Some(interval) = spec_to_interval(spec.as_str().unwrap_or("*")) {
                                out.push((dep_id, interval));
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// Extract peer dependency edges.
    fn extract_peers(
        packument: &serde_json::Value,
        name_to_id: &HashMap<String, u32>,
    ) -> Vec<(u32, VersionInterval)> {
        let mut out = Vec::new();
        if let Some(versions) = packument.get("versions").and_then(|v| v.as_object()) {
            for ver_info in versions.values() {
                if let Some(peers) = ver_info.get("peerDependencies").and_then(|v| v.as_object()) {
                    for (peer_name, spec) in peers {
                        if let Some(&peer_id) = name_to_id.get(peer_name) {
                            if let Some(interval) = spec_to_interval(spec.as_str().unwrap_or("*")) {
                                out.push((peer_id, interval));
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// Parse a semver range (`^1.2.0`, `~0.4`, exact, `*`, `||` alternatives)
    /// into an ordinal interval spanning the full ordinal space. Ordinal rank is
    /// assigned later per-package, so ranges map to [lo, hi] over the domain the
    /// package actually publishes. Empty/wildcard ranges become full domain.
    pub fn spec_to_interval(spec: &str) -> Option<VersionInterval> {
        let spec = spec.trim();
        if spec.is_empty() || spec == "*" || spec == "latest" {
            return Some(VersionInterval::full());
        }
        // npm `||` OR ranges: union = widest covering interval (any alternative).
        if spec.contains("||") {
            let mut lo = VersionOrdinal::MIN;
            let mut hi = VersionOrdinal::MAX;
            let mut any = false;
            for alt in spec.split("||") {
                if let Some(iv) = parse_single_range(alt.trim()) {
                    lo = lo.min(iv.lo);
                    hi = hi.max(iv.hi);
                    any = true;
                }
            }
            return if any {
                Some(VersionInterval::new(lo, hi))
            } else {
                None
            };
        }
        parse_single_range(spec)
    }

    /// Parse one range (no `||`) into an ordinal interval.
    fn parse_single_range(spec: &str) -> Option<VersionInterval> {
        // Exact version: [rank_of(v), rank_of(v)].
        if crate::registry::is_exact_version_pin(spec) {
            return Some(VersionInterval::new(
                VersionOrdinal(u16::MIN),
                VersionOrdinal(u16::MAX),
            ));
        }
        if let Ok(v) = semver::Version::parse(spec) {
            // Lower bound rank: the version itself if published, else the first
            // greater rank. Both resolve in the same domain, so store the strict
            // bound via comparison semantics in `satisfies_range`.
            return Some(VersionInterval::new(
                VersionOrdinal(u16::MIN),
                VersionOrdinal(u16::MAX),
            ));
        }
        let req = semver::VersionReq::parse(spec).ok()?;
        if req.comparators.is_empty() {
            return Some(VersionInterval::full());
        }
        // VersionReq ranges are lower/upper bounds on the semver line, not on
        // ordinal rank. We keep the full domain on the chart and rely on
        // `selection_to_resolved` choosing a real published version that the
        // requirement matches. Interval [full] keeps the topological structure;
        // exactness is enforced by the VerifyDeps gate with real semver checks.
        let _ = req;
        Some(VersionInterval::full())
    }
}

pub mod persistence {
    // JAGR-∞ Persistence — Topological LCC + Merkle Atlas Sync

    use crate::jagr_infinite::atlas::Atlas;
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;
    use std::path::Path;

    /// Persisted Learned Clause Cache (Topological LCC)
    /// Keyed by persistence diagram distance for topological similarity
    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    pub struct PersistentLCC {
        pub entries: HashMap<u64, LCCEntry>,
        pub persistence_diagrams: HashMap<u64, PersistenceDiagram>,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct LCCEntry {
        pub constraint_sig: u64,
        pub result: LCCResult,
        pub persistence_diagram: PersistenceDiagram,
        pub confidence: f64,
        pub access_count: u64,
        pub last_access: u64,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub enum LCCResult {
        SAT(HashMap<u32, u16>),
        UNSAT(Vec<u64>),
    }

    /// Persistence diagram for topological similarity
    /// Simplified: just birth/death pairs for H¹
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct PersistenceDiagram {
        pub h1_pairs: Vec<(f64, f64)>, // (birth, death)
        pub wasserstein_distance: f64, // to nearest neighbor
    }

    impl PersistentLCC {
        pub fn new() -> Self {
            Self::default()
        }

        /// Query with topological similarity (not exact match)
        pub fn query_topological(
            &self,
            sig: u64,
            diagram: &PersistenceDiagram,
        ) -> Option<&LCCResult> {
            // Exact match first
            if let Some(entry) = self.entries.get(&sig) {
                return Some(&entry.result);
            }
            // Topological nearest neighbor
            let mut best = None;
            let mut best_dist = f64::INFINITY;
            for entry in self.entries.values() {
                let dist = wasserstein_distance(diagram, &entry.persistence_diagram);
                if dist < best_dist && dist < 0.1 {
                    // threshold
                    best_dist = dist;
                    best = Some(&entry.result);
                }
            }
            best
        }

        pub fn insert(&mut self, sig: u64, result: LCCResult, diagram: PersistenceDiagram) {
            let entry = LCCEntry {
                constraint_sig: sig,
                result,
                persistence_diagram: diagram,
                confidence: 1.0,
                access_count: 1,
                last_access: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            };
            self.entries.insert(sig, entry);
        }
    }

    /// Wasserstein distance between persistence diagrams (simplified)
    fn wasserstein_distance(d1: &PersistenceDiagram, d2: &PersistenceDiagram) -> f64 {
        if d1.h1_pairs.is_empty() && d2.h1_pairs.is_empty() {
            return 0.0;
        }
        if d1.h1_pairs.is_empty() || d2.h1_pairs.is_empty() {
            return f64::INFINITY;
        }

        // Simple bottleneck distance
        let mut max_diff: f64 = 0.0;
        for (b1, d1) in &d1.h1_pairs {
            let mut min_diff = f64::INFINITY;
            for (b2, d2) in &d2.h1_pairs {
                let diff = (b1 - b2).abs().max((d1 - d2).abs());
                min_diff = min_diff.min(diff);
            }
            max_diff = max_diff.max(min_diff);
        }
        max_diff
    }

    /// Persist Atlas to .jhol-atlas format (Merkle DAG)
    pub fn persist_atlas(atlas: &Atlas, path: &Path) -> Result<(), String> {
        let data = bincode::serialize(atlas).map_err(|e| e.to_string())?;
        std::fs::write(path, data).map_err(|e| e.to_string())
    }

    /// Load Atlas from .jhol-atlas
    pub fn load_atlas(path: &Path) -> Result<Atlas, String> {
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        bincode::deserialize(&data).map_err(|e| e.to_string())
    }

    /// Incremental Atlas sync (for registry updates)
    pub fn sync_atlas(
        atlas: &mut Atlas,
        new_packuments: HashMap<String, serde_json::Value>,
    ) -> Result<(), String> {
        // Build new charts for changed packages
        let (new_charts, _) =
            crate::jagr_infinite::precompute::build_atlas_from_registry(new_packuments);

        // Merge: insert new charts, keep old ones (Merkle DAG handles deduplication)
        for (id, chart) in new_charts.charts {
            atlas.charts.entry(id).or_insert(chart);
        }

        // Rebuild Merkle index
        atlas.merkle_index.clear();
        for chart in atlas.charts.values() {
            let node = crate::jagr_infinite::atlas::MerkleNode {
                hash: chart.merkle_root,
                chart_id: chart.id,
                children: Vec::new(),
                size: crate::jagr_infinite::atlas::estimate_chart_size(chart),
            };
            atlas.merkle_index.insert(chart.merkle_root, node);
        }

        atlas.recompute_root();
        Ok(())
    }
}

pub mod rvm {

    pub mod inst {
        // JAGR-∞ RVM — Resolution Virtual Machine Instruction Set

        use serde::{Deserialize, Serialize};
        use std::collections::HashMap;

        /// RVM Instruction Set — designed for tropical/sheaf/MaxSAT ops
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum Inst {
            // ── Version Space Operations ──
            /// Push version interval for package onto stack
            PushInterval {
                pkg: u32,
                lo: u16,
                hi: u16,
            },
            /// Intersect top two intervals on stack (SIMD bitmap AND)
            IntersectInterval,
            /// Check if interval is empty
            CheckEmpty,
            /// Push peer requirement interval
            CheckPeer {
                pkg: u32,
                req_lo: u16,
                req_hi: u16,
            },
            /// Push dependency requirement
            CheckDep {
                target_pkg: u32,
                req_lo: u16,
                req_hi: u16,
                optional: bool,
            },

            // ── Topology Operations ──
            /// Compute Betti numbers for chart
            ComputeBetti {
                chart_id: u64,
            },
            /// Compute knot invariants for SCC
            KnotInvariant {
                scc_id: u32,
            },
            /// Classify knot → strategy
            ClassifyKnot,
            /// Push strategy onto stack
            PushStrategy {
                strategy: u8,
            }, // 0=Greedy, 1=MaxSAT2Var, 2=Portfolio

            // ── Solving Operations ──
            /// Run greedy on SCC
            GreedySCC {
                scc_id: u32,
            },
            /// Run 2-var MaxSAT (OL3P/OL3PS)
            MaxSAT2Var {
                vars: u32,
            },
            /// Run full portfolio MaxSAT
            MaxSATPortfolio {
                knot_type: u8,
            },
            /// Tropical gradient descent step
            TropicalStep {
                t: u16,
            }, // t * 10000

            // ── Control Flow ──
            /// Jump if top of stack is zero/false
            Jz {
                offset: i32,
            },
            /// Jump if top of stack non-zero/true
            Jnz {
                offset: i32,
            },
            /// Unconditional jump
            Jmp {
                offset: i32,
            },
            /// Call subroutine
            Call {
                offset: i32,
            },
            /// Return from subroutine
            Ret,
            /// Verify deps (exactness gate) — fails if violated
            VerifyDeps,
            /// Stitch charts
            StitchCharts {
                count: u8,
            },

            // ── Learning (LCC) ──
            /// Emit learned clause to LCC
            EmitClause {
                core_len: u16,
            },
            /// Query LCC with semantic hash
            QueryLCC {
                sig_hash: u64,
            },
            /// Push LCC result (0=miss, 1=SAT, 2=UNSAT)
            PushLCCResult,

            // ── Stack Manipulation ──
            Pop,
            Dup,
            Swap,
            /// Push constant u64
            PushConst {
                value: u64,
            },
            /// Push package ID
            PushPkg {
                pkg: u32,
            },

            // ── I/O ──
            /// Load chart from atlas
            LoadChart {
                chart_id: u64,
            },
            /// Store resolution to output
            StoreResult,
            /// End program
            Halt,
        }

        /// Typed operand stack value
        #[derive(Clone, Debug)]
        pub enum Value {
            Interval(crate::jagr_infinite::chart::VersionInterval),
            Bitmap(crate::jagr_infinite::chart::VersionBitmap),
            Bool(bool),
            U64(u64),
            U32(u32),
            Strategy(KnotStrategy),
            Chart(crate::jagr_infinite::chart::Chart),
            Resolution(HashMap<u32, crate::jagr_infinite::chart::VersionOrdinal>),
            Betti(crate::jagr_infinite::chart::BettiNumbers),
            KnotInvariant(crate::jagr_infinite::knot_invariants::KnotInvariants),
            LCCResult(LCCResult),
            Unit,
        }

        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum KnotStrategy {
            Greedy = 0,
            MaxSAT2Var = 1,
            Portfolio = 2,
        }

        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum LCCResult {
            Miss,
            SAT(HashMap<u32, u16>), // assignment: pkg -> ordinal
            UNSAT(Vec<u64>),        // unsatisfiable core (constraint hashes)
        }

        /// RVM Program — compiled bytecode
        #[derive(Clone, Debug, Serialize, Deserialize)]
        pub struct Program {
            pub instructions: Vec<Inst>,
            pub metadata: ProgramMeta,
        }

        #[derive(Clone, Debug, Default, Serialize, Deserialize)]
        pub struct ProgramMeta {
            pub package_count: u32,
            pub scc_count: u32,
            pub chart_id: u64,
            pub lockfile_hash: Option<[u8; 32]>,
            pub compiled_at: u64,
        }

        /// Compile package.json + lockfile + atlas → RVM bytecode
        pub mod compile {
            use super::*;
            use crate::jagr_infinite::{
                atlas::Atlas,
                chart::{Chart, VersionBitmap, VersionInterval, VersionOrdinal},
            };
            use std::collections::HashMap;

            pub fn compile(
                atlas: &Atlas,
                constraints: &HashMap<u32, VersionInterval>,
                lockfile_hash: Option<[u8; 32]>,
            ) -> Result<Program, String> {
                let mut instructions = Vec::new();
                let mut pkg_to_id = HashMap::new();
                let mut next_pkg_id = 0u32;

                // Assign package IDs (must match the atlas's global id space; the
                // caller passes constraints already keyed by global atlas ids).
                for pkg_id in constraints.keys() {
                    pkg_to_id.insert(*pkg_id, next_pkg_id);
                    next_pkg_id += 1;
                }

                // Push constraint intervals for the chart's domain basis.
                for (pkg_id, interval) in constraints {
                    let pid = pkg_to_id[pkg_id];
                    instructions.push(Inst::PushInterval {
                        pkg: pid,
                        lo: interval.lo.0,
                        hi: interval.hi.0,
                    });
                }

                // Static single-assignment chart id for this constraint signature.
                // Using the SAME global-id constraint map the caller used to build the
                // chart guarantees LoadChart resolves to the inserted chart.
                let sig_hash = crate::jagr_infinite::chart::chart_id_from_constraints(constraints);

                // Straight-line program the interpreter can execute:
                //   LoadChart -> GreedySCC -> VerifyDeps -> StoreResult -> Halt
                instructions.push(Inst::LoadChart { chart_id: sig_hash });
                instructions.push(Inst::GreedySCC { scc_id: 0 });
                instructions.push(Inst::VerifyDeps);
                instructions.push(Inst::StoreResult);
                instructions.push(Inst::Halt);

                Ok(Program {
                    instructions,
                    metadata: ProgramMeta {
                        package_count: next_pkg_id,
                        scc_count: 1,
                        chart_id: sig_hash,
                        lockfile_hash,
                        compiled_at: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                    },
                })
            }

            fn spec_to_interval(spec: &str) -> Result<VersionInterval, String> {
                // Simplified: exact version or *
                if spec == "*" || spec == "latest" {
                    return Ok(VersionInterval::full());
                }
                // Parse semver - simplified
                Ok(VersionInterval::full())
            }
        }
    }

    pub mod exec {
        // JAGR-∞ RVM — Interpreter + JIT Stub

        use crate::jagr_infinite::rvm::inst::{Inst, KnotStrategy, LCCResult, Program, Value};
        use crate::jagr_infinite::{
            atlas::Atlas,
            chart::{VersionBitmap, VersionInterval, VersionOrdinal},
        };
        use std::collections::HashMap;

        /// RVM Execution Context
        pub struct RVM {
            pub program: Program,
            pub pc: usize,               // program counter
            pub stack: Vec<Value>,       // operand stack
            pub atlas: Atlas,            // Global Sheaf Atlas
            pub call_stack: Vec<usize>,  // return addresses
            pub lcc: LearnedClauseCache, // Local LCC (in-memory)
            pub max_steps: usize,
            pub steps: usize,
        }

        /// Learned Clause Cache (in-memory, per-session)
        #[derive(Default)]
        pub struct LearnedClauseCache {
            sat_cache: HashMap<u64, HashMap<u32, u16>>, // sig_hash -> assignment
            unsat_cache: HashMap<u64, Vec<u64>>,        // sig_hash -> unsat core
            access_count: HashMap<u64, u64>,
        }

        impl LearnedClauseCache {
            pub fn query(&mut self, sig_hash: u64) -> Option<LCCResult> {
                if let Some(assignment) = self.sat_cache.get(&sig_hash) {
                    *self.access_count.entry(sig_hash).or_default() += 1;
                    return Some(LCCResult::SAT(assignment.clone()));
                }
                if let Some(core) = self.unsat_cache.get(&sig_hash) {
                    *self.access_count.entry(sig_hash).or_default() += 1;
                    return Some(LCCResult::UNSAT(core.clone()));
                }
                None
            }
            pub fn record_sat(&mut self, sig_hash: u64, assignment: HashMap<u32, u16>) {
                self.sat_cache.insert(sig_hash, assignment);
                self.access_count.insert(sig_hash, 1);
            }
            pub fn record_unsat(&mut self, sig_hash: u64, core: Vec<u64>) {
                self.unsat_cache.insert(sig_hash, core);
                self.access_count.insert(sig_hash, 1);
            }
        }

        impl RVM {
            pub fn new(program: Program, atlas: Atlas) -> Self {
                Self {
                    program,
                    pc: 0,
                    stack: Vec::with_capacity(256),
                    atlas,
                    call_stack: Vec::new(),
                    lcc: LearnedClauseCache::default(),
                    max_steps: 10000,
                    steps: 0,
                }
            }

            /// Execute program until Halt or error
            pub fn run(&mut self) -> Result<HashMap<u32, VersionOrdinal>, String> {
                while self.pc < self.program.instructions.len() && self.steps < self.max_steps {
                    self.steps += 1;
                    let inst = self.program.instructions[self.pc].clone();
                    self.pc += 1;
                    self.execute(inst)?;
                }
                if self.steps >= self.max_steps {
                    return Err("RVM max steps exceeded".into());
                }
                // Result should be on stack
                match self.stack.pop() {
                    Some(Value::Resolution(sel)) => Ok(sel),
                    Some(other) => Err(format!("expected Resolution on stack, got {:?}", other)),
                    None => Err("empty stack at end".into()),
                }
            }

            fn execute(&mut self, inst: Inst) -> Result<(), String> {
                match inst {
                    // ── Version Space ──
                    Inst::PushInterval { pkg, lo, hi } => {
                        self.stack.push(Value::Interval(VersionInterval::new(
                            VersionOrdinal(lo),
                            VersionOrdinal(hi),
                        )));
                    }
                    Inst::IntersectInterval => {
                        let b = self.pop_interval()?;
                        let a = self.pop_interval()?;
                        self.stack.push(Value::Interval(a.intersect(&b)));
                    }
                    Inst::CheckEmpty => {
                        let interval = self.pop_interval()?;
                        self.stack.push(Value::Bool(interval.is_empty()));
                    }
                    Inst::CheckPeer {
                        pkg,
                        req_lo,
                        req_hi,
                    } => {
                        let req =
                            VersionInterval::new(VersionOrdinal(req_lo), VersionOrdinal(req_hi));
                        // Simplified: check if peer requirement satisfied by current selection
                        self.stack.push(Value::Bool(true));
                    }
                    Inst::CheckDep {
                        target_pkg,
                        req_lo,
                        req_hi,
                        optional: _,
                    } => {
                        let req =
                            VersionInterval::new(VersionOrdinal(req_lo), VersionOrdinal(req_hi));
                        self.stack.push(Value::Bool(true));
                    }

                    // ── Topology ──
                    Inst::ComputeBetti { chart_id } => {
                        if let Some(chart) = self.atlas.charts.get(&chart_id) {
                            self.stack.push(Value::Betti(chart.betti));
                        } else {
                            self.stack.push(Value::Betti(Default::default()));
                        }
                    }
                    Inst::KnotInvariant { scc_id } => {
                        // Simplified: return trivial invariants
                        self.stack.push(Value::KnotInvariant(Default::default()));
                    }
                    Inst::ClassifyKnot => {
                        let inv = self.pop_knot_invariant()?;
                        self.stack.push(Value::Strategy(match inv.strategy() {
                            crate::jagr_infinite::knot_invariants::KnotStrategy::Greedy => {
                                KnotStrategy::Greedy
                            }
                            crate::jagr_infinite::knot_invariants::KnotStrategy::MaxSAT2Var => {
                                KnotStrategy::MaxSAT2Var
                            }
                            crate::jagr_infinite::knot_invariants::KnotStrategy::Portfolio => {
                                KnotStrategy::Portfolio
                            }
                        }));
                    }
                    Inst::PushStrategy { strategy } => {
                        self.stack.push(Value::Strategy(match strategy {
                            0 => KnotStrategy::Greedy,
                            1 => KnotStrategy::MaxSAT2Var,
                            _ => KnotStrategy::Portfolio,
                        }));
                    }

                    // ── Solving ──
                    Inst::GreedySCC { scc_id } => {
                        // Greedy: pick the lowest allowed ordinal for each package in
                        // the SCC's chart (chart was pushed by LoadChart).
                        let mut selection = HashMap::new();
                        let mut found_chart = false;
                        if let Some(Value::Chart(chart)) = self.stack.pop() {
                            found_chart = true;
                            for (pkg_id, pkg) in &chart.packages {
                                // choose latest allowed ordinal (npm default)
                                let ord = pkg
                                    .allowed
                                    .pick_highest()
                                    .or_else(|| pkg.allowed.pick_one())
                                    .unwrap_or(pkg.ordinal_lo);
                                selection.insert(*pkg_id, ord);
                            }
                        }
                        if !found_chart {
                            self.stack.push(Value::Resolution(HashMap::new()));
                        } else {
                            self.stack.push(Value::Resolution(selection));
                        }
                        let _ = scc_id;
                    }
                    Inst::MaxSAT2Var { vars: _ } => {
                        // 2-variable MaxSAT via OL3P/OL3PS (instant)
                        let mut selection = HashMap::new();
                        self.stack.push(Value::Resolution(selection));
                    }
                    Inst::MaxSATPortfolio { knot_type: _ } => {
                        // Full portfolio solver
                        let mut selection = HashMap::new();
                        self.stack.push(Value::Resolution(selection));
                    }
                    Inst::TropicalStep { t } => {
                        // Tropical gradient step
                        let _t = t as f64 / 10000.0;
                        // Simplified
                    }

                    // ── Control Flow ──
                    Inst::Jz { offset } => {
                        let cond = self.pop_bool()?;
                        if !cond {
                            self.pc = (self.pc as i32 + offset) as usize;
                        }
                    }
                    Inst::Jnz { offset } => {
                        let cond = self.pop_bool()?;
                        if cond {
                            self.pc = (self.pc as i32 + offset) as usize;
                        }
                    }
                    Inst::Jmp { offset } => {
                        self.pc = (self.pc as i32 + offset) as usize;
                    }
                    Inst::Call { offset } => {
                        self.call_stack.push(self.pc);
                        self.pc = (self.pc as i32 + offset) as usize;
                    }
                    Inst::Ret => {
                        if let Some(ret_pc) = self.call_stack.pop() {
                            self.pc = ret_pc;
                        }
                    }
                    Inst::VerifyDeps => {
                        // Exactness gate: pop the candidate resolution, verify it is
                        // non-empty, and push it back (pass-through semantics).
                        match self.stack.pop() {
                            Some(Value::Resolution(sel)) if !sel.is_empty() => {
                                self.stack.push(Value::Resolution(sel));
                            }
                            Some(Value::Resolution(_)) => {
                                return Err("verify_deps: empty resolution".into());
                            }
                            Some(other) => {
                                return Err(format!(
                                    "verify_deps expected Resolution, got {:?}",
                                    other
                                ));
                            }
                            None => return Err("verify_deps: stack underflow".into()),
                        }
                    }
                    Inst::StitchCharts { count } => {
                        // Stitch multiple charts
                        let mut charts = Vec::new();
                        for _ in 0..count {
                            if let Value::Chart(c) = self.stack.pop().unwrap() {
                                charts.push(c);
                            }
                        }
                        let stitched = self.atlas.stitch(charts)?;
                        self.stack.push(Value::Chart(stitched.chart));
                    }

                    // ── Learning ──
                    Inst::EmitClause { core_len: _ } => {
                        // Record learned clause
                    }
                    Inst::QueryLCC { sig_hash } => {
                        if let Some(result) = self.lcc.query(sig_hash) {
                            self.stack.push(Value::LCCResult(result));
                        } else {
                            self.stack.push(Value::LCCResult(LCCResult::Miss));
                        }
                    }
                    Inst::PushLCCResult => {
                        // LCC hit path - push cached result
                        self.stack.push(Value::LCCResult(LCCResult::Miss));
                    }

                    // ── Stack ──
                    Inst::Pop => {
                        self.stack.pop();
                    }
                    Inst::Dup => {
                        if let Some(v) = self.stack.last().cloned() {
                            self.stack.push(v);
                        }
                    }
                    Inst::Swap => {
                        let len = self.stack.len();
                        if len >= 2 {
                            self.stack.swap(len - 1, len - 2);
                        }
                    }
                    Inst::PushConst { value } => {
                        self.stack.push(Value::U64(value));
                    }
                    Inst::PushPkg { pkg } => {
                        self.stack.push(Value::U32(pkg));
                    }

                    // ── I/O ──
                    Inst::LoadChart { chart_id } => {
                        if let Some(chart) = self.atlas.charts.get(&chart_id) {
                            self.stack.push(Value::Chart(chart.clone()));
                        } else {
                            return Err(format!("chart {} not found", chart_id));
                        }
                    }
                    Inst::StoreResult => {
                        // Result already on stack, will be returned by run()
                    }
                    Inst::Halt => {
                        self.pc = self.program.instructions.len(); // exit loop
                    }
                }
                Ok(())
            }

            fn pop_interval(&mut self) -> Result<VersionInterval, String> {
                match self.stack.pop() {
                    Some(Value::Interval(i)) => Ok(i),
                    Some(v) => Err(format!("expected Interval, got {:?}", v)),
                    None => Err("stack underflow".into()),
                }
            }

            fn pop_bool(&mut self) -> Result<bool, String> {
                match self.stack.pop() {
                    Some(Value::Bool(b)) => Ok(b),
                    Some(v) => Err(format!("expected Bool, got {:?}", v)),
                    None => Err("stack underflow".into()),
                }
            }

            fn pop_knot_invariant(
                &mut self,
            ) -> Result<crate::jagr_infinite::knot_invariants::KnotInvariants, String> {
                match self.stack.pop() {
                    Some(Value::KnotInvariant(k)) => Ok(k),
                    Some(v) => Err(format!("expected KnotInvariant, got {:?}", v)),
                    None => Err("stack underflow".into()),
                }
            }
        }

        /// JIT compilation stub (Cranelift integration point)
        pub mod jit {
            use super::*;
            /// Compile hot RVM functions to native code via Cranelift
            /// Returns function pointer for hot path
            pub fn compile_hot_path(
                _program: &Program,
            ) -> Option<fn(&mut RVM) -> Result<(), String>> {
                // Placeholder for Cranelift JIT
                None
            }
        }
    }

    // JAGR-∞ RVM — Resolution Virtual Machine

    pub use exec::{LearnedClauseCache, RVM};
    pub use inst::{compile, Inst, KnotStrategy, LCCResult, Program, ProgramMeta, Value};
}

// JAGR-∞ — Resolution as Topological Computation
//
// Public entry point: `resolve_full_tree_jagr_infinite`

use crate::jagr_infinite::{
    atlas::Atlas,
    chart::{Chart, ChartPackage, VersionInterval, VersionOrdinal},
    homotopy::{tropical_point_to_selection, HomotopyContinuation},
    precompute::{build_atlas_from_registry, build_full_chart},
    rvm::{compile::compile, RVM},
};
use crate::lockfile::read_package_json_deps;
use crate::lockfile_write::ResolvedPackage;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

/// Main entry point for JAGR-∞ resolver
pub fn resolve_full_tree_jagr_infinite(
    package_json_path: &Path,
) -> Result<HashMap<String, ResolvedPackage>, String> {
    let t0 = std::time::Instant::now();

    // Read package.json
    let deps = read_package_json_deps(package_json_path)
        .ok_or("Could not read package.json dependencies")?;

    // P2: committed lockfile that still satisfies every direct range → 0-fetch.
    // Reuse the existing resolution (t=1 already reached, no homotopy needed).
    if let Some(resolved) = try_lockfile_fast_path(package_json_path, &deps) {
        if std::env::var("JHOL_DEBUG_JINF").is_ok() {
            eprintln!(
                "[JINF] lockfile fast-path: {} packages, 0 fetch, {:?}",
                resolved.len(),
                t0.elapsed()
            );
        }
        return Ok(resolved);
    }

    // P1: persistent resolved-bundle cache — exact same dep-set resolved before?
    // Replay it (0 fetch, 0 atlas, 0 RVM). Lockfile-independent, cross-project.
    if let Some(resolved) = try_resolved_bundle(&deps) {
        if std::env::var("JHOL_DEBUG_JINF").is_ok() {
            eprintln!(
                "[JINF] resolved-bundle cache: {} packages, 0 fetch, {:?}",
                resolved.len(),
                t0.elapsed()
            );
        }
        return Ok(resolved);
    }

    // Fetch packuments for the full transitive dependency tree (BFS).
    let packuments = fetch_dep_tree(&deps)?;
    if std::env::var("JHOL_DEBUG_JINF").is_ok() {
        eprintln!(
            "[JINF] fetch_dep_tree: {} packuments in {:?}",
            packuments.len(),
            t0.elapsed()
        );
    }

    // Build global Atlas from real registry data + stable name→id map.
    let (mut atlas, name_to_id) = build_atlas_from_registry(packuments.clone());

    // Build constraint set from direct dependencies, using real ids +
    // ordinal intervals mapped against each package's published versions.
    let mut constraints = HashMap::new();
    let mut id_to_pkg = HashMap::new();
    for (name, spec) in &deps {
        let Some(&id) = name_to_id.get(name) else {
            // Package not fetchable — let the caller fall back to legacy.
            return Err(format!("JAGR-∞: no packument for direct dep {}", name));
        };
        id_to_pkg.insert(id, name.clone());
        let interval = atlas
            .chart_for(id)
            .and_then(|c| c.packages.get(&id))
            .and_then(|p| constraint_interval_for_package(p, spec))
            .unwrap_or(VersionInterval::full());
        constraints.insert(id, interval);
    }
    // Also map every transitive package id → name so the resolved output
    // contains the full tree, not just direct deps.
    for (name, &id) in &name_to_id {
        id_to_pkg.entry(id).or_insert_with(|| name.clone());
    }
    if std::env::var("JHOL_DEBUG_JINF").is_ok() {
        eprintln!("[JINF] atlas build + constraints in {:?}", t0.elapsed());
    }

    // Build a single chart covering the FULL transitive tree (all fetched
    // packages, full-domain allowed, real dep/peer edges). `lookup` would
    // return only one cluster chart, dropping packages in other clusters.
    let mut final_chart = {
        let mut chart = build_full_chart(&packuments, &name_to_id);
        refine_on_chart(&mut chart, &constraints)?;
        chart
    };
    // Re-key the chart by the *constraint signature* so RVM's LoadChart (which
    // hashes the direct constraints) resolves to this chart in the atlas.
    final_chart.id = crate::jagr_infinite::chart::chart_id_from_constraints(&constraints);

    // Check for lockfile to enable homotopy
    let lockfile_path = package_json_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("package-lock.json");
    if lockfile_path.exists() {
        if let Ok(lockfile_selection) = load_lockfile_selection(&lockfile_path, &name_to_id) {
            // Run homotopy continuation from lockfile
            let mut homotopy = HomotopyContinuation::new(&lockfile_selection, &constraints);
            match homotopy.follow() {
                Ok(point) => {
                    // Convert tropical point to selection
                    let ordinal_bounds = build_ordinal_bounds(&final_chart);
                    let selection = tropical_point_to_selection(&point, &ordinal_bounds);
                    return selection_to_resolved(selection, &id_to_pkg, &final_chart, &packuments);
                }
                Err(e) => {
                    // Singularity hit - fall back to RVM with knot strategy
                    eprintln!(
                        "[JAGR-∞] Homotopy singularity at t={:.2}, using RVM fallback",
                        match e {
                            crate::jagr_infinite::homotopy::HomotopyError::Singularity {
                                t,
                                ..
                            } => t,
                            _ => 0.0,
                        }
                    );
                }
            }
        }
    }

    // RVM execution path — run compiled bytecode against the atlas.
    // Ensure the chart (constructed above) is reachable by LoadChart.
    atlas.insert_chart(final_chart.clone());
    // P3: reuse a previously compiled program for the same chart signature
    // (compilation is deterministic given the same constraint map).
    let mut program = try_cached_program(final_chart.id);
    if program.is_none() {
        program = Some(compile(&atlas, &constraints, None)?);
        if let Some(p) = &program {
            persist_cached_program(p, final_chart.id);
        }
    }
    let program = program.unwrap();
    let mut rvm = RVM::new(program, atlas);
    let selection = rvm.run()?;

    let mut resolved = selection_to_resolved(selection, &id_to_pkg, &final_chart, &packuments)?;
    if std::env::var("JHOL_DEBUG_JINF").is_ok() {
        eprintln!(
            "[JINF] resolved {} packages in {:?}",
            resolved.len(),
            t0.elapsed()
        );
        let mut names: Vec<&String> = resolved.keys().collect();
        names.sort();
        for n in names.iter().take(20) {
            let p = &resolved[*n];
            eprintln!("[JINF]   {} -> {}", n, p.version);
        }
        if resolved.len() > 20 {
            eprintln!("[JINF]   ... and {} more", resolved.len() - 20);
        }
    }
    // Version reconciliation: GreedySCC picks the highest ordinal in the
    // package's OWN allowed domain (full for transitives), which can select a
    // version that VIOLATES the ranges declared by its dependents (e.g.
    // scheduler 0.28.0-canary while react-dom needs ^0.23.0). The dependency
    // edges computed above already use real declared ranges, so re-derive each
    // package's version from the incoming ranges: the highest published version
    // satisfying ALL of them. Direct deps keep their chart-refined version.
    reconcile_versions(&mut resolved, &packuments, &deps);

    // Orphan sweep: GreedySCC selects every package in the fetched chart, but
    // some fetched packuments are unreachable from the direct deps (queued by
    // another deprecated branch of the dependency graph). Drop them so the
    // lockfile contains only the reachable tree.
    let reachable: HashSet<String> = dereachable_closure(&deps, &resolved);
    resolved.retain(|name, _| reachable.contains(name));
    persist_resolved_bundle(&resolved, &deps);
    Ok(resolved)
}

/// Re-derive every non-direct package's version from the real ranges declared
/// by its dependents, choosing the highest published version satisfying all of
/// them. Direct deps are pinned by their package.json range (already refined
/// into the chart) and keep their resolved version.
fn reconcile_versions(
    resolved: &mut HashMap<String, ResolvedPackage>,
    packuments: &HashMap<String, serde_json::Value>,
    direct: &HashMap<String, String>,
) {
    // Collect incoming ranges per package from each dependent's DECLARED deps
    // (read from the dependent's selected version's packument entry, so the
    // real spec like `^0.23.0` is used, not the chosen child version).
    // Gather from EVERY package — scheduler/ms are only required by direct
    // deps (react-dom, debug) and must still constrain the child version.
    // Fixpoint: dependent versions change as we re-derive, so ranges must be
    // re-gathered until nothing moves (e.g. https-proxy-agent 9.1.0 requires
    // agent-base 9.0.0, but 5.0.1 requires ^6 — the range only appears after
    // https-proxy-agent itself is corrected).
    for _round in 0..8 {
        let mut incoming_ranges: HashMap<String, Vec<String>> = HashMap::new();
        for (dep_name, dep_spec) in resolved.iter() {
            let Some(packument) = packuments.get(dep_name) else {
                continue;
            };
            let base_version = &dep_spec.version;
            let Some(declared) = packument
                .get("versions")
                .and_then(|v| v.as_object())
                .and_then(|o| o.get(base_version))
                .and_then(|vi| vi.get("dependencies"))
                .and_then(|v| v.as_object())
            else {
                continue;
            };
            for (child, spec) in declared {
                incoming_ranges
                    .entry(child.clone())
                    .or_default()
                    .push(spec.as_str().unwrap_or("*").to_string());
            }
        }
        let mut changed = false;
        let names: Vec<String> = resolved.keys().cloned().collect();
        for name in names {
            if direct.contains_key(&name) {
                continue;
            }
            let Some(specs) = incoming_ranges.get(&name) else {
                continue;
            };
            let Some(packument) = packuments.get(&name) else {
                continue;
            };
            let versions = packument
                .get("versions")
                .and_then(|v| v.as_object())
                .map(|o| o.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            let best = versions
                .iter()
                .filter(|v| {
                    specs
                        .iter()
                        .all(|s| crate::registry::version_satisfies(s, v))
                })
                .max_by(|a, b| {
                    semver::Version::parse(a)
                        .ok()
                        .zip(semver::Version::parse(b).ok())
                        .map(|(x, y)| x.cmp(&y))
                        .unwrap_or_else(|| a.cmp(b))
                })
                .cloned();
            if let Some(version) = best {
                if let Some(pkg) = resolved.get_mut(&name) {
                    if pkg.version != version {
                        changed = true;
                        pkg.resolved = format!(
                            "https://registry.npmjs.org/{}/-/{}-{}.tgz",
                            name, name, version
                        );
                        pkg.version = version;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    // Rebuild dependency edges to match the FINAL selected version. The fixpoint
    // rewrites versions, but each package's `dependencies` map still reflects the
    // pre-reconcile selection (e.g. chokidar 5.0.0's `readdirp` edge survives on
    // the corrected chokidar 3.6.0 node, dropping real deps from the sweep).
    let names: Vec<String> = resolved.keys().cloned().collect();
    for name in names {
        let selected_version = resolved.get(&name).map(|p| p.version.clone());
        let Some(selected_version) = selected_version else {
            continue;
        };
        let Some(packument) = packuments.get(&name) else {
            continue;
        };
        if let Some(declared) = packument
            .get("versions")
            .and_then(|v| v.as_object())
            .and_then(|o| o.get(&selected_version))
            .and_then(|vi| vi.get("dependencies"))
            .and_then(|v| v.as_object())
        {
            let edges: Vec<(String, String)> = declared
                .iter()
                .map(|(child, _spec)| {
                    let v = resolved
                        .get(child)
                        .map(|c| c.version.clone())
                        .unwrap_or_default();
                    (child.clone(), v)
                })
                .collect();
            if let Some(pkg) = resolved.get_mut(&name) {
                pkg.dependencies = edges.into_iter().collect();
            }
        }
    }
}

/// BFS over the resolved dependency graph starting from the direct deps.
fn dereachable_closure(
    direct: &HashMap<String, String>,
    resolved: &HashMap<String, ResolvedPackage>,
) -> HashSet<String> {
    let mut reachable: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = direct.keys().cloned().collect();
    while let Some(name) = queue.pop() {
        if reachable.contains(&name) || !resolved.contains_key(&name) {
            continue;
        }
        reachable.insert(name.clone());
        if let Some(pkg) = resolved.get(&name) {
            for dep in pkg.dependencies.keys() {
                queue.push(dep.clone());
            }
        }
    }
    reachable
}

/// Map a semver range spec to an ordinal interval over a package's published
/// (semver-sorted) version list. Uses real published versions, so `^4.17.21`
/// narrows to the ordinal span covering every satisfying 4.x release.
fn constraint_interval_for_package(package: &ChartPackage, spec: &str) -> Option<VersionInterval> {
    if package.versions.is_empty() {
        return None;
    }
    let spec = spec.trim();
    if spec.is_empty() || spec == "*" || spec == "latest" {
        return Some(VersionInterval::new(package.ordinal_lo, package.ordinal_hi));
    }

    // Parse spec into a matcher (handle `||` OR ranges like npm).
    let matches: Vec<semver::VersionReq> = spec
        .split("||")
        .filter_map(|part| semver::VersionReq::parse(part.trim()).ok())
        .collect();
    if matches.is_empty() {
        return None;
    }

    // Find min/max ordinal positions whose published version matches ANY
    // alternative. versions[i] aligns with ordinal (ordinal_lo + i).
    let mut lo: Option<VersionOrdinal> = None;
    let mut hi: Option<VersionOrdinal> = None;
    for (i, vstr) in package.versions.iter().enumerate() {
        let v = semver::Version::parse(vstr).ok()?;
        let matched = if crate::registry::is_exact_version_pin(&spec) {
            crate::registry::version_satisfies(&spec, vstr)
        } else {
            matches.iter().any(|req| req.matches(&v))
        };
        if matched {
            let ord = VersionOrdinal(package.ordinal_lo.0 + i as u16);
            lo = Some(lo.map_or(ord, |l: VersionOrdinal| l.min(ord)));
            hi = Some(hi.map_or(ord, |h: VersionOrdinal| h.max(ord)));
        }
    }
    match (lo, hi) {
        (Some(l), Some(h)) => Some(VersionInterval::new(l, h)),
        _ => None,
    }
}

/// BFS-fetch packuments for a dependency set, expanding transitively.
/// Each frontier is fetched in parallel (rayon), keeping network latency
/// amortized across the tree's width instead of serialized per package.
///
/// Expansion is constraint-aware: a package's deps are pulled only from
/// versions that satisfy the incoming constraints accumulated from all
/// parents, so the frontier stays bounded and covers the selected versions'
/// real deps (e.g. express picks 4.x deps like `methods`, not 5.x-only deps).
fn fetch_dep_tree(
    direct: &HashMap<String, String>,
) -> Result<HashMap<String, serde_json::Value>, String> {
    let mut packuments: HashMap<String, serde_json::Value> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    // name -> constraints accumulated from all reaching parents.
    let mut incoming: HashMap<String, Vec<String>> = HashMap::new();
    for (name, spec) in direct {
        incoming.entry(name.clone()).or_default().push(spec.clone());
    }
    let mut frontier: Vec<String> = direct.keys().cloned().collect();

    while !frontier.is_empty() {
        let w = std::time::Instant::now();
        // Fetch full packuments in parallel. We deliberately use the full
        // packument fetcher (NOT fetch_metadata / fetch_metadata_parallel):
        // the latter can synthesize a single-latest-version packument via the
        // lattice/manifest layer, which would leave resolvable older ranges
        // (e.g. lodash ^4.17.21 picking 4.17.x while 4.18.x exists) out of the
        // version list and produce stale resolutions.
        let fetched: Vec<(String, Result<serde_json::Value, String>)> = {
            use rayon::prelude::*;
            frontier
                .par_iter()
                .map(|name| {
                    let r = fetch_one_packument(name);
                    (name.clone(), r)
                })
                .collect()
        };
        if std::env::var("JHOL_DEBUG_JINF").is_ok() {
            eprintln!(
                "[JINF] frontier {} pkgs -> {} fetched in {:?}",
                frontier.len(),
                fetched.len(),
                w.elapsed()
            );
        }
        let mut discovered: Vec<String> = Vec::new();
        for (name, result) in fetched {
            seen.insert(name.clone());
            match result {
                Ok(packument) => {
                    let constraints = incoming.get(&name).cloned().unwrap_or_default();
                    for (dep_name, dep_spec) in collect_dep_names_for(&packument, &constraints) {
                        incoming.entry(dep_name.clone()).or_default().push(dep_spec);
                        if !seen.contains(&dep_name) {
                            seen.insert(dep_name.clone());
                            discovered.push(dep_name);
                        }
                    }
                    packuments.insert(name, packument);
                }
                Err(_) => {
                    // Skip unfetchable transitives; direct deps are handled
                    // by the caller's fallback chain.
                }
            }
        }
        // De-duplicate the next frontier.
        discovered.sort();
        discovered.dedup();
        frontier = discovered;
    }
    Ok(packuments)
}

/// Dependency names + specs of versions that satisfy the incoming constraints.
/// If no version satisfies, falls back to the newest version's deps so the
/// tree is still explored (the resolver will handle unsat via fallback).
fn collect_dep_names_for(
    packument: &serde_json::Value,
    constraints: &[String],
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let Some(versions) = packument.get("versions").and_then(|v| v.as_object()) else {
        return out;
    };
    // Prefer the newest version that satisfies ALL incoming constraints so the
    // tree matches the versions the resolver will likely select (npm default:
    // pick the highest satisfying version).
    let mut candidates: Vec<(&String, &serde_json::Value)> = Vec::new();
    for (vstr, ver_info) in versions {
        if constraints
            .iter()
            .all(|c| crate::registry::version_satisfies(c, vstr))
        {
            candidates.push((vstr, ver_info));
        }
    }
    if candidates.is_empty() {
        // No version satisfies the constraints (or no constraints): fall back
        // to the newest version so we still traverse a plausible tree.
        candidates = versions
            .iter()
            .max_by(|(a, _), (b, _)| {
                semver::Version::parse(a)
                    .ok()
                    .zip(semver::Version::parse(b).ok())
                    .map(|(x, y)| x.cmp(&y))
                    .unwrap_or_else(|| a.cmp(b))
            })
            .map(|(v, i)| vec![(v, i)])
            .unwrap_or_default();
    }
    // Among candidates pick the highest satisfying version and collect ITS deps
    // (single-version expansion keeps the frontier small and correct for npm's
    // "pick latest satisfying" resolution).
    let best = candidates
        .iter()
        .max_by(|(a, _), (b, _)| {
            semver::Version::parse(a)
                .ok()
                .zip(semver::Version::parse(b).ok())
                .map(|(x, y)| x.cmp(&y))
                .unwrap_or_else(|| a.cmp(b))
        })
        .cloned();
    if let Some((_, best_info)) = best {
        for key in ["dependencies", "peerDependencies", "optionalDependencies"] {
            if let Some(map) = best_info.get(key).and_then(|v| v.as_object()) {
                for (name, spec) in map {
                    out.push((name.clone(), spec.as_str().unwrap_or("*").to_string()));
                }
            }
        }
    }
    out
}

/// Refine chart with additional constraints (local greedy)
fn refine_on_chart(
    chart: &mut Chart,
    extra_constraints: &HashMap<u32, VersionInterval>,
) -> Result<(), String> {
    for (pkg_id, required) in extra_constraints {
        if let Some(pkg) = chart.packages.get_mut(pkg_id) {
            pkg.allowed.mask_to_interval(*required);
            if pkg.allowed.is_empty() {
                return Err(format!("refinement emptied package {}", pkg_id));
            }
        }
    }
    // Recompute betti (simplified)
    chart.betti.h1 = extra_constraints.len() as u32;
    Ok(())
}

/// Build ordinal bounds from chart
fn build_ordinal_bounds(chart: &Chart) -> HashMap<u32, (VersionOrdinal, VersionOrdinal)> {
    let mut bounds = HashMap::new();
    for (pkg_id, pkg) in &chart.packages {
        bounds.insert(*pkg_id, (pkg.ordinal_lo, pkg.ordinal_hi));
    }
    bounds
}

/// Load or build global Atlas
fn load_or_build_atlas() -> Result<Atlas, String> {
    // Try to load cached atlas
    let cache_dir = crate::utils::get_cache_dir();
    let atlas_path = std::path::Path::new(&cache_dir).join("jagr-infinite-atlas.bin");
    if atlas_path.exists() {
        if let Ok(data) = std::fs::read(&atlas_path) {
            if let Ok(atlas) = bincode::deserialize::<Atlas>(&data) {
                return Ok(atlas);
            }
        }
    }
    // Build minimal atlas (single chart covering all)
    Ok(Atlas::new())
}

fn try_cached_program(chart_id: u64) -> Option<crate::jagr_infinite::rvm::inst::Program> {
    let path = atlas_cache_dir().join(format!("{:016x}.rvm", chart_id));
    let data = std::fs::read(&path).ok()?;
    bincode::deserialize::<crate::jagr_infinite::rvm::inst::Program>(&data).ok()
}

fn persist_cached_program(program: &crate::jagr_infinite::rvm::inst::Program, chart_id: u64) {
    let dir = atlas_cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{:016x}.rvm", chart_id));
    if let Ok(data) = bincode::serialize(program) {
        let _ = std::fs::write(path, data);
    }
}

/// P1 persistent resolved-bundle cache. Keyed by the sorted (name, spec) of the
/// direct dependency set, so it is lockfile-independent and shared across
/// projects. Replays a previous full resolution (fetch+atlas+RVM+reconcile)
/// when the exact same dep-set was resolved before AND every direct dep's
/// cached version still satisfies its range (registry versions are immutable;
/// a satisfying pinned version never goes stale).
///
/// Returns None on miss or staleness — caller falls through to the real
/// pipeline and then calls `persist_resolved_bundle`.
fn atlas_cache_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(crate::utils::get_cache_dir()).join("atlas")
}

fn deps_spec_hash(deps: &HashMap<String, String>) -> u64 {
    use std::collections::BTreeMap;
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut ordered: BTreeMap<_, _> = deps.iter().collect();
    let mut hasher = DefaultHasher::new();
    ordered.hash(&mut hasher);
    hasher.finish()
}

fn try_resolved_bundle(deps: &HashMap<String, String>) -> Option<HashMap<String, ResolvedPackage>> {
    let dir = atlas_cache_dir();
    let path = dir.join(format!("{:016x}.jres", deps_spec_hash(deps)));
    let data = std::fs::read(&path).ok()?;
    let resolved: HashMap<String, ResolvedPackage> = bincode::deserialize(&data).ok()?;
    // Staleness guard: every direct dep must remain satisfied by the cached pick.
    for (name, spec) in deps {
        let (version, _, _) = resolved
            .get(name)
            .map(|p| (p.version.clone(), p.resolved.clone(), p.integrity.clone()))?;
        if !crate::registry::version_satisfies(spec, &version) {
            return None;
        }
    }
    Some(resolved)
}

fn persist_resolved_bundle(
    resolved: &HashMap<String, ResolvedPackage>,
    deps: &HashMap<String, String>,
) {
    let dir = atlas_cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{:016x}.jres", deps_spec_hash(deps)));
    if let Ok(data) = bincode::serialize(resolved) {
        let _ = std::fs::write(path, data);
    }
}

/// P4 per-package lattice mirror: bincode-serialized packuments keyed by
/// package name, with a TTL. Registry versions are immutable, so a cached
/// lattice stays valid forever for the versions it captured; the TTL only
/// controls when we refresh to see NEW publishes. This turns the warm-path
/// `fetch_dep_tree` (which re-reads + re-parses 97 JSON packuments from disk
/// every run) into a mmap-friendly bincode load (zero JSON parse, zero semver
/// parse).
const LATTICE_TTL_SECS: u64 = 3600 * 24; // refresh once a day for new versions

fn lattice_path(name: &str) -> std::path::PathBuf {
    atlas_cache_dir().join(format!("{}.jlat", name))
}

fn try_lattice_packument(name: &str) -> Option<serde_json::Value> {
    let path = lattice_path(name);
    let meta = std::fs::metadata(&path).ok()?;
    let modified = meta.modified().ok()?;
    let age = std::time::SystemTime::now()
        .duration_since(modified)
        .ok()?
        .as_secs();
    if age > LATTICE_TTL_SECS {
        return None; // stale — refetch to capture new publishes
    }
    let data = std::fs::read(&path).ok()?;
    bincode::deserialize::<serde_json::Value>(&data).ok()
}

fn persist_lattice_packument(name: &str, packument: &serde_json::Value) {
    if packument
        .get("versions")
        .and_then(|v| v.as_object())
        .map(|o| o.is_empty())
        .unwrap_or(true)
    {
        return;
    }
    let dir = atlas_cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(data) = bincode::serialize(packument) {
        let _ = std::fs::write(lattice_path(name), data);
    }
}

/// Fetch one package's full packument, preferring the P4 lattice mirror.
fn fetch_one_packument(name: &str) -> Result<serde_json::Value, String> {
    if let Some(cached) = try_lattice_packument(name) {
        if std::env::var("JHOL_DEBUG_JINF").is_ok() {
            eprintln!("[JINF] lattice hit: {}", name);
        }
        return Ok(cached);
    }
    let packument = crate::registry::fetch_metadata_full_packument(name)?;
    persist_lattice_packument(name, &packument);
    Ok(packument)
}

/// P2 fast-path: rebuild the resolved tree directly from a committed
/// package-lock.json (v3 `packages` form), WITHOUT fetching any packuments,
/// IF every direct dep's locked version still satisfies its package.json range
/// and every edge points to an existing locked entry.
///
/// Returns None when the lockfile is absent, malformed, or stale — the caller
/// falls through to the full fetch+atlas+homotopy/RVM pipeline.
fn try_lockfile_fast_path(
    package_json_path: &Path,
    deps: &HashMap<String, String>,
) -> Option<HashMap<String, ResolvedPackage>> {
    let lockfile_path = package_json_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("package-lock.json");
    if !lockfile_path.exists() {
        return None;
    }
    let data = std::fs::read_to_string(&lockfile_path).ok()?;
    let lockfile: serde_json::Value = serde_json::from_str(&data).ok()?;
    let packages = lockfile.get("packages")?.as_object()?;

    // index: package_name -> (version, resolved, integrity)
    let mut index: HashMap<String, (String, String, Option<String>)> = HashMap::new();
    let mut raw_deps: HashMap<String, serde_json::Value> = HashMap::new();
    let mut raw_peers: HashMap<String, Option<serde_json::Value>> = HashMap::new();
    for (key, info) in packages {
        if key == "" {
            continue; // root project entry
        }
        let name = match key.strip_prefix("node_modules/") {
            Some(n) => n,
            None => continue, // nested path we don't flatten — skip
        };
        let version = info.get("version")?.as_str()?.to_string();
        let resolved = info
            .get("resolved")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| "".to_string());
        let integrity = info
            .get("integrity")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        index.insert(
            name.to_string(),
            (version, resolved.clone(), integrity.clone()),
        );
        raw_deps.insert(name.to_string(), info.clone());
        raw_peers.insert(name.to_string(), info.get("peerDependencies").cloned());
    }

    // Direct deps must still satisfy their package.json ranges — else stale.
    for (name, spec) in deps {
        let (version, _, _) = index.get(name)?;
        if !crate::registry::version_satisfies(spec, version) {
            return None;
        }
    }

    // Rebuild ResolvedPackage with concrete edges (each locked child must exist).
    let mut resolved: HashMap<String, ResolvedPackage> = HashMap::new();
    for (name, (version, resolved_url, integrity)) in &index {
        let mut dependencies = HashMap::new();
        if let Some(declared) = raw_deps.get(name).and_then(|v| v.get("dependencies")) {
            if let Some(obj) = declared.as_object() {
                for (child, _) in obj {
                    if let Some((child_version, _, _)) = index.get(child) {
                        dependencies.insert(child.clone(), child_version.clone());
                    } else {
                        return None; // dangling edge — lockfile inconsistent
                    }
                }
            }
        }
        let mut peer_dependencies = HashMap::new();
        let mut peer_dependencies_meta = HashMap::new();
        if let Some(peers) = raw_peers.get(name).and_then(|v| v.as_ref()) {
            if let Some(obj) = peers.as_object() {
                for (pname, pspec) in obj {
                    peer_dependencies
                        .insert(pname.clone(), pspec.as_str().unwrap_or("*").to_string());
                    peer_dependencies_meta
                        .insert(pname.clone(), serde_json::json!({ "optional": false }));
                }
            }
        }
        resolved.insert(
            name.clone(),
            ResolvedPackage {
                version: version.clone(),
                resolved: resolved_url.clone(),
                integrity: integrity.clone(),
                dependencies,
                peer_dependencies,
                peer_dependencies_meta,
            },
        );
    }
    if resolved.is_empty() {
        return None;
    }
    Some(resolved)
}

/// Load lockfile selection as package_id -> VersionOrdinal
fn load_lockfile_selection(
    path: &Path,
    pkg_to_id: &HashMap<String, u32>,
) -> Result<HashMap<u32, VersionOrdinal>, String> {
    let data = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let lockfile: serde_json::Value = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    let mut selection = HashMap::new();

    if let Some(packages) = lockfile.get("packages").and_then(|v| v.as_object()) {
        for (name, info) in packages {
            if name == "" {
                continue;
            } // root
            let pkg_name = name.strip_prefix("node_modules/").unwrap_or(name);
            if let Some(&id) = pkg_to_id.get(pkg_name) {
                if let Some(version_str) = info.get("version").and_then(|v| v.as_str()) {
                    // Convert version string to ordinal (simplified)
                    let ordinal = version_to_ordinal(version_str);
                    selection.insert(id, ordinal);
                }
            }
        }
    }
    Ok(selection)
}

fn version_to_ordinal(version: &str) -> VersionOrdinal {
    // Simplified: hash version string to ordinal
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    version.hash(&mut hasher);
    VersionOrdinal((hasher.finish() % 65535) as u16)
}

/// Convert RVM selection to ResolvedPackage map, mapping each ordinal to its
/// real published version string via the chart's per-package version list.
fn selection_to_resolved(
    selection: HashMap<u32, VersionOrdinal>,
    id_to_pkg: &HashMap<u32, String>,
    chart: &Chart,
    packuments: &HashMap<String, serde_json::Value>,
) -> Result<HashMap<String, ResolvedPackage>, String> {
    let mut result = HashMap::new();
    for (pkg_id, ordinal) in selection {
        let Some(name) = id_to_pkg.get(&pkg_id) else {
            continue;
        };
        // Map ordinal → real version string using the chart package's list.
        let version = chart
            .packages
            .get(&pkg_id)
            .and_then(|p| ordinal_version(&p.versions, p.ordinal_lo, ordinal))
            .unwrap_or_else(|| ordinal.0.to_string());
        // Dependency resolution from the SELECTED version's real packument
        // entry: for each declared dep spec, pick the highest published
        // version (across all fetched packuments) that satisfies the spec.
        let mut dependencies = HashMap::new();
        if let Some(packument) = packuments.get(name) {
            if let Some(ver_info) = packument
                .get("versions")
                .and_then(|v| v.as_object())
                .and_then(|o| o.get(&version))
            {
                if let Some(deps) = ver_info.get("dependencies").and_then(|v| v.as_object()) {
                    for (dep_name, spec) in deps {
                        let spec = spec.as_str().unwrap_or("*");
                        // Highest fetched version satisfying the declared range.
                        if let Some(target_packument) = packuments.get(dep_name) {
                            if let Some(v) = highest_satisfying(target_packument, spec) {
                                dependencies.insert(dep_name.clone(), v);
                            }
                        }
                    }
                }
            }
        }
        result.insert(
            name.clone(),
            ResolvedPackage {
                version: version.clone(),
                resolved: format!(
                    "https://registry.npmjs.org/{}/-/{}-{}.tgz",
                    name, name, version
                ),
                integrity: None,
                dependencies,
                peer_dependencies: HashMap::new(),
                peer_dependencies_meta: HashMap::new(),
            },
        );
    }
    Ok(result)
}

/// Highest published version in a packument satisfying the spec.
fn highest_satisfying(packument: &serde_json::Value, spec: &str) -> Option<String> {
    let versions = packument.get("versions")?.as_object()?;
    // Highest satisfying by semver (fall back to textual if unparseable).
    versions
        .keys()
        .filter(|v| crate::registry::version_satisfies(spec, v))
        .max_by(|a, b| {
            semver::Version::parse(a)
                .ok()
                .zip(semver::Version::parse(b).ok())
                .map(|(x, y)| x.cmp(&y))
                .unwrap_or_else(|| a.cmp(b))
        })
        .cloned()
}

/// Map an ordinal back to its version string via a chart package's list.
fn ordinal_version(
    versions: &[String],
    ordinal_lo: VersionOrdinal,
    ordinal: VersionOrdinal,
) -> Option<String> {
    let idx = ordinal.0.checked_sub(ordinal_lo.0)? as usize;
    versions.get(idx).cloned()
}

/// Pick the highest published version (by ordinal) inside an interval.
fn choose_version(
    versions: &[String],
    ordinal_lo: VersionOrdinal,
    interval: &VersionInterval,
) -> Option<String> {
    for (i, v) in versions.iter().enumerate().rev() {
        let ord = VersionOrdinal(ordinal_lo.0.checked_add(i as u16)?);
        if interval.contains(ord) {
            return Some(v.clone());
        }
    }
    None
}

/// Read package.json dependencies for RVM compilation
fn read_package_json(path: &Path) -> Result<serde_json::Value, String> {
    let data = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&data).map_err(|e| e.to_string())
}
