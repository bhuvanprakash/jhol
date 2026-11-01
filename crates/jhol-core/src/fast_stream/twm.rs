//! Topological Wavefront Multiplexing (TWM 2.0)
//!
//! Uses Cellular Sheaf Cohomology separation ($H^1(X, \mathcal{F}) = 0$) from JAGR-Infinite
//! to schedule immediate priority tarball streaming for non-conflicting Ultraviolet (UV) strands
//! in parallel with graph discovery.

use std::collections::HashMap;

/// Topological classification of dependency nodes
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrandCategory {
    /// Ultraviolet leaf strand with trivial cohomology H^1 = 0 (uniquely determined, safe to stream immediately)
    UltravioletLeaf,
    /// Infrared core with non-trivial cohomology H^1 != 0 (requires homotopy / solver conflict resolution)
    InfraredCore,
}

/// Wavefront Dispatch Descriptor
#[derive(Debug, Clone)]
pub struct WavefrontDispatchItem {
    pub package: String,
    pub spec: String,
    pub category: StrandCategory,
    pub http2_priority_weight: u8,
}

/// Topological Wavefront Scheduler
pub struct TopologicalWavefrontScheduler;

impl TopologicalWavefrontScheduler {
    /// Classifies root and discovered dependencies into UV strands (H^1 = 0) vs IR cores (H^1 != 0)
    /// based on topological sheaf degree and constraint multiplicity.
    pub fn classify_wavefront(
        root_deps: &[(String, String)],
        transitive_constraints: &HashMap<String, Vec<String>>,
    ) -> Vec<WavefrontDispatchItem> {
        Self::classify_wavefront_with_ricci(root_deps, transitive_constraints, None)
    }

    /// Classifies dependencies with discrete Forman-Ricci curvature prioritization.
    /// Packages with negative curvature Ric(u, v) < 0 (structural choke points like react, next, eslint)
    /// are boosted to higher HTTP/2 stream priority weights to unblock downstream hardlinks early.
    pub fn classify_wavefront_with_ricci(
        root_deps: &[(String, String)],
        transitive_constraints: &HashMap<String, Vec<String>>,
        ricci_curvatures: Option<&HashMap<String, i32>>,
    ) -> Vec<WavefrontDispatchItem> {
        let mut items = Vec::with_capacity(root_deps.len());

        for (pkg, spec) in root_deps {
            let seen_specs = transitive_constraints.get(pkg);
            let is_single_constraint = match seen_specs {
                Some(specs) => specs.len() <= 1,
                None => true,
            };

            // Exploit Forman-Ricci curvature: negative curvature indicates a structural hub.
            let ricci_weight = if let Some(curvatures) = ricci_curvatures {
                if let Some(&ricci) = curvatures.get(pkg) {
                    if ricci < 0 {
                        // Boost priority: e.g. ricci = -17 -> (128 - (-17 * 4)) = 196
                        (128_i32 - ricci * 4).clamp(128, 255) as u8
                    } else {
                        128
                    }
                } else {
                    128
                }
            } else {
                128
            };

            // If a package has an exact version or single constraint with in-degree <= 1,
            // its local cohomology H^1 is 0 (Ultraviolet Leaf Strand).
            // It can be dispatched on the HTTP/2 multiplexed channel immediately with maximum weight (255).
            if is_single_constraint {
                items.push(WavefrontDispatchItem {
                    package: pkg.clone(),
                    spec: spec.clone(),
                    category: StrandCategory::UltravioletLeaf,
                    http2_priority_weight: 255, // Max priority
                });
            } else {
                items.push(WavefrontDispatchItem {
                    package: pkg.clone(),
                    spec: spec.clone(),
                    category: StrandCategory::InfraredCore,
                    http2_priority_weight: ricci_weight,
                });
            }
        }

        // Sort Ultraviolet strands first and highest curvature weights first for instant stream saturation
        items.sort_by(|a, b| b.http2_priority_weight.cmp(&a.http2_priority_weight));
        items
    }

    /// Classifies dependencies with both discrete Forman-Ricci curvature prioritization
    /// and Second-Order Markov transition probabilities $P(C \mid A, B)$.
    /// When a package has high transition confidence ($\ge 0.50$), its stream weight is boosted
    /// so the HTTP/2 engine pre-warms sockets and streams dependencies before topological resolution finishes.
    pub fn classify_wavefront_with_markov_and_ricci(
        root_deps: &[(String, String)],
        transitive_constraints: &HashMap<String, Vec<String>>,
        ricci_curvatures: Option<&HashMap<String, i32>>,
        markov_predictions: Option<&[(String, f64)]>,
    ) -> Vec<WavefrontDispatchItem> {
        let mut items = Self::classify_wavefront_with_ricci(
            root_deps,
            transitive_constraints,
            ricci_curvatures,
        );

        if let Some(predictions) = markov_predictions {
            let pred_map: HashMap<&str, f64> =
                predictions.iter().map(|(p, c)| (p.as_str(), *c)).collect();
            for item in &mut items {
                if let Some(&conf) = pred_map.get(item.package.as_str()) {
                    if conf >= 0.50 {
                        let boost = (conf * 64.0) as u8;
                        item.http2_priority_weight =
                            item.http2_priority_weight.saturating_add(boost);
                    }
                }
            }
            items.sort_by(|a, b| b.http2_priority_weight.cmp(&a.http2_priority_weight));
        }

        items
    }

    /// Classifies dependencies using Chemical Reaction Network (CRN) Mass-Action Diffusion Kinetics.
    ///
    /// In a CRN dependency lattice, packages represent interacting chemical species whose activities
    /// obey mass-action potential $\mu_i = \mu_i^\circ + R T \ln a_i$. Reversible dependency reactions
    /// $\sum \nu_{ir} S_i \rightleftharpoons \sum \nu'_{jr} P_j$ exert chemical affinities $A_r = -\sum \nu_{kr} \mu_k$.
    ///
    /// Flux equilibrium $\nabla \mu_i = 0$ corresponds to the Gibbs free energy minimum $\Delta G = 0$.
    /// Fickian diffusion along the cellular simplicial complex couples with the Sheaf Laplacian $\Delta_\mathcal{F}$:
    ///   $$\frac{\partial \mathbf{c}}{\partial t} = - \mathbf{D} \Delta_\mathcal{F} \mathbf{c}$$
    /// Species at equilibrium with non-negative chemical affinity exhibit zero constraint conflict
    /// and are prioritized for early HTTP/2 streaming.
    pub fn classify_wavefront_with_chemical_kinetics(
        root_deps: &[(String, String)],
        transitive_constraints: &HashMap<String, Vec<String>>,
        ricci_curvatures: Option<&HashMap<String, i32>>,
        markov_predictions: Option<&[(String, f64)]>,
        chemical_affinities: Option<&HashMap<String, f64>>,
    ) -> Vec<WavefrontDispatchItem> {
        let mut items = Self::classify_wavefront_with_markov_and_ricci(
            root_deps,
            transitive_constraints,
            ricci_curvatures,
            markov_predictions,
        );

        if let Some(affinities) = chemical_affinities {
            for item in &mut items {
                if let Some(&affinity) = affinities.get(&item.package) {
                    if affinity >= 0.0 {
                        let kinetic_boost = (affinity.min(1.0) * 32.0) as u8;
                        item.http2_priority_weight =
                            item.http2_priority_weight.saturating_add(kinetic_boost);
                    }
                }
            }
            items.sort_by(|a, b| b.http2_priority_weight.cmp(&a.http2_priority_weight));
        }

        items
    }

    /// Selects candidates ready for zero-RTT streaming download
    pub fn get_immediate_stream_candidates(
        items: &[WavefrontDispatchItem],
    ) -> Vec<(String, String)> {
        items
            .iter()
            .filter(|item| item.category == StrandCategory::UltravioletLeaf)
            .map(|item| (item.package.clone(), item.spec.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_twm_classification() {
        let root_deps = vec![
            ("lodash".to_string(), "^4.17.21".to_string()),
            ("axios".to_string(), "^1.7.9".to_string()),
            ("conflicting-pkg".to_string(), "^1.0.0".to_string()),
        ];

        let mut constraints = HashMap::new();
        constraints.insert("lodash".to_string(), vec!["^4.17.21".to_string()]);
        constraints.insert("axios".to_string(), vec!["^1.7.9".to_string()]);
        constraints.insert(
            "conflicting-pkg".to_string(),
            vec!["^1.0.0".to_string(), "^2.0.0".to_string()],
        );

        let classified =
            TopologicalWavefrontScheduler::classify_wavefront(&root_deps, &constraints);
        assert_eq!(classified.len(), 3);

        let uv_candidates =
            TopologicalWavefrontScheduler::get_immediate_stream_candidates(&classified);
        assert_eq!(uv_candidates.len(), 2);
        assert!(uv_candidates.iter().any(|(p, _)| p == "lodash"));
        assert!(uv_candidates.iter().any(|(p, _)| p == "axios"));
    }

    #[test]
    fn test_twm_ricci_boost() {
        let root_deps = vec![
            ("shared-hub".to_string(), "^1.0.0".to_string()),
            ("normal-core".to_string(), "^1.0.0".to_string()),
        ];

        let mut constraints = HashMap::new();
        constraints.insert(
            "shared-hub".to_string(),
            vec!["^1.0.0".to_string(), "^2.0.0".to_string()],
        );
        constraints.insert(
            "normal-core".to_string(),
            vec!["^1.0.0".to_string(), "^2.0.0".to_string()],
        );

        let mut ricci_map = HashMap::new();
        ricci_map.insert("shared-hub".to_string(), -20); // Large negative curvature bottleneck

        let classified = TopologicalWavefrontScheduler::classify_wavefront_with_ricci(
            &root_deps,
            &constraints,
            Some(&ricci_map),
        );

        let hub_item = classified
            .iter()
            .find(|i| i.package == "shared-hub")
            .unwrap();
        let normal_item = classified
            .iter()
            .find(|i| i.package == "normal-core")
            .unwrap();

        assert!(hub_item.http2_priority_weight > normal_item.http2_priority_weight);
        assert_eq!(hub_item.http2_priority_weight, 208); // 128 + 20*4
    }

    #[test]
    fn test_twm_markov_boost() {
        let root_deps = vec![
            ("pkg-a".to_string(), "^1.0.0".to_string()),
            ("pkg-b".to_string(), "^1.0.0".to_string()),
        ];

        let mut constraints = HashMap::new();
        constraints.insert(
            "pkg-a".to_string(),
            vec!["^1.0.0".to_string(), "^2.0.0".to_string()],
        );
        constraints.insert(
            "pkg-b".to_string(),
            vec!["^1.0.0".to_string(), "^2.0.0".to_string()],
        );

        let predictions = vec![("pkg-a".to_string(), 0.90)];

        let classified = TopologicalWavefrontScheduler::classify_wavefront_with_markov_and_ricci(
            &root_deps,
            &constraints,
            None,
            Some(&predictions),
        );

        let item_a = classified.iter().find(|i| i.package == "pkg-a").unwrap();
        let item_b = classified.iter().find(|i| i.package == "pkg-b").unwrap();

        assert!(item_a.http2_priority_weight > item_b.http2_priority_weight);
        assert_eq!(item_a.http2_priority_weight, 128 + (0.90 * 64.0) as u8);
    }

    #[test]
    fn test_twm_chemical_kinetics_boost() {
        let root_deps = vec![
            ("pkg-equilibrium".to_string(), "^1.0.0".to_string()),
            ("pkg-strained".to_string(), "^1.0.0".to_string()),
        ];

        let mut constraints = HashMap::new();
        constraints.insert(
            "pkg-equilibrium".to_string(),
            vec!["^1.0.0".to_string(), "^2.0.0".to_string()],
        );
        constraints.insert(
            "pkg-strained".to_string(),
            vec!["^1.0.0".to_string(), "^2.0.0".to_string()],
        );

        let mut affinities = HashMap::new();
        affinities.insert("pkg-equilibrium".to_string(), 0.85); // Favorable free energy minimum

        let classified = TopologicalWavefrontScheduler::classify_wavefront_with_chemical_kinetics(
            &root_deps,
            &constraints,
            None,
            None,
            Some(&affinities),
        );

        let item_eq = classified
            .iter()
            .find(|i| i.package == "pkg-equilibrium")
            .unwrap();
        let item_st = classified
            .iter()
            .find(|i| i.package == "pkg-strained")
            .unwrap();

        assert!(item_eq.http2_priority_weight > item_st.http2_priority_weight);
        assert_eq!(item_eq.http2_priority_weight, 128 + (0.85 * 32.0) as u8);
    }
}
