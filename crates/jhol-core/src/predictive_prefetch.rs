//! Phase 15: Predictive Prefetching with Markov Chain Analysis
//!
//! Learns from global install patterns to prefetch likely dependencies.
//! "Ghost installs" - dependencies already in cache when you need them.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
pub struct MarkovPredictor {
    // (pkg1, pkg2) -> { next_pkg -> probability }
    transitions: HashMap<(String, String), HashMap<String, f64>>,
    // Count of observations for smoothing
    observations: HashMap<(String, String), u64>,
}

impl Default for MarkovPredictor {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkovPredictor {
    pub fn new() -> Self {
        Self {
            transitions: HashMap::new(),
            observations: HashMap::new(),
        }
    }

    /// Record an install sequence (pkg1, pkg2) -> next_pkg
    pub fn observe(&mut self, pkg1: &str, pkg2: &str, next_pkg: &str) {
        let key = (pkg1.to_string(), pkg2.to_string());

        self.transitions
            .entry(key.clone())
            .or_default()
            .entry(next_pkg.to_string())
            .and_modify(|count| *count += 1.0)
            .or_insert(1.0);

        self.observations
            .entry(key)
            .and_modify(|count| *count += 1)
            .or_insert(1);
    }

    /// Predict next packages given history
    pub fn predict_next(&self, history: &[String]) -> Vec<(String, f64)> {
        if history.len() < 2 {
            return Vec::new();
        }

        let key = (
            history[history.len() - 2].clone(),
            history[history.len() - 1].clone(),
        );

        if let Some(probs) = self.transitions.get(&key) {
            let total_obs = *self.observations.get(&key).unwrap_or(&1) as f64;

            let mut predictions: Vec<(String, f64)> = probs
                .iter()
                .map(|(pkg, &count)| (pkg.clone(), count / total_obs))
                .collect();

            // Sort by probability
            predictions.sort_by(|a, b| {
                // Handle NaN: treat as greater than all finite values
                if a.1.is_nan() && b.1.is_nan() {
                    std::cmp::Ordering::Equal
                } else if a.1.is_nan() {
                    std::cmp::Ordering::Greater
                } else if b.1.is_nan() {
                    std::cmp::Ordering::Less
                } else {
                    b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                }
            });

            // Return top 5
            predictions.into_iter().take(5).collect()
        } else {
            Vec::new()
        }
    }

    /// Predict next packages with discrete Forman-Ricci curvature bottleneck weighting:
    /// Score(p) = P(p | history) * exp(2.5 * |min(0, Ric(p))| / max(1, |Ric_min|))
    /// Packages located at geometric bottlenecks are prioritized for background pre-fetching.
    pub fn predict_next_with_ricci(
        &self,
        history: &[String],
        ricci_curvatures: Option<&HashMap<String, i32>>,
    ) -> Vec<(String, f64)> {
        let mut predictions = self.predict_next(history);
        if let Some(curvatures) = ricci_curvatures {
            let min_curv = curvatures
                .values()
                .copied()
                .min()
                .unwrap_or(0)
                .min(0)
                .abs()
                .max(1) as f64;
            for (pkg, prob) in &mut predictions {
                if let Some(&curv) = curvatures.get(pkg) {
                    if curv < 0 {
                        let normalized_bottleneck = (curv.abs() as f64) / min_curv;
                        let weight = (2.5 * normalized_bottleneck).exp();
                        *prob *= weight;
                    }
                }
            }
            predictions.sort_by(|a, b| {
                if a.1.is_nan() && b.1.is_nan() {
                    std::cmp::Ordering::Equal
                } else if a.1.is_nan() {
                    std::cmp::Ordering::Greater
                } else if b.1.is_nan() {
                    std::cmp::Ordering::Less
                } else {
                    b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                }
            });
        }
        predictions
    }

    /// Load predictor from disk
    pub fn load(path: &PathBuf) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::new());
        }

        let mut file = File::open(path).map_err(|e| format!("Failed to open predictor: {}", e))?;

        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|e| format!("Failed to read predictor: {}", e))?;

        let predictor: MarkovPredictor = bincode::deserialize(&data)
            .map_err(|e| format!("Failed to deserialize predictor: {}", e))?;

        Ok(predictor)
    }

    /// Save predictor to disk
    pub fn save(&self, path: &PathBuf) -> Result<(), String> {
        let serialized = bincode::serialize(self)
            .map_err(|e| format!("Failed to serialize predictor: {}", e))?;

        fs::create_dir_all(path.parent().unwrap())
            .map_err(|e| format!("Failed to create dir: {}", e))?;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(path)
            .map_err(|e| format!("Failed to open file: {}", e))?;

        file.write_all(&serialized)
            .map_err(|e| format!("Failed to write: {}", e))?;

        Ok(())
    }

    /// Merge with another predictor (for federated learning)
    pub fn merge(&mut self, other: &MarkovPredictor) {
        for (key, probs) in &other.transitions {
            let entry = self.transitions.entry(key.clone()).or_default();
            for (pkg, &prob) in probs {
                entry
                    .entry(pkg.clone())
                    .and_modify(|p| *p += prob)
                    .or_insert(prob);
            }
        }

        for (key, &count) in &other.observations {
            self.observations
                .entry(key.clone())
                .and_modify(|c| *c += count)
                .or_insert(count);
        }
    }

    /// Get statistics
    pub fn stats(&self) -> PredictorStats {
        PredictorStats {
            transition_count: self.transitions.len(),
            total_observations: self.observations.values().sum(),
            unique_packages: self
                .transitions
                .values()
                .flat_map(|probs| probs.keys())
                .collect::<std::collections::HashSet<_>>()
                .len(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictorStats {
    pub transition_count: usize,
    pub total_observations: u64,
    pub unique_packages: usize,
}

/// Background prefetch daemon
pub struct PrefetchDaemon {
    predictor: MarkovPredictor,
    cache_dir: PathBuf,
    pending_downloads: Vec<String>,
}

impl PrefetchDaemon {
    pub fn new(predictor: MarkovPredictor, cache_dir: PathBuf) -> Self {
        Self {
            predictor,
            cache_dir,
            pending_downloads: Vec::new(),
        }
    }

    /// Prefetch likely packages based on install history
    pub fn prefetch_likely(&mut self, history: &[String]) -> Vec<String> {
        let predictions = self.predictor.predict_next(history);

        let mut prefetched = Vec::new();
        for (pkg, probability) in predictions {
            if probability > 0.3 {
                // Only prefetch if >30% likely
                if !self.is_cached(&pkg) {
                    self.pending_downloads.push(pkg.clone());
                    prefetched.push(pkg);
                }
            }
        }

        prefetched
    }

    /// Check if package is cached
    fn is_cached(&self, package: &str) -> bool {
        let cache_path = self.cache_dir.join(package);
        cache_path.exists()
    }

    /// Run background downloads (non-blocking)
    pub fn run_background_downloads(&self) {
        // Use rayon for parallel downloads
        self.pending_downloads.par_iter().for_each(|pkg| {
            // In real implementation, would download package
            // For now, just simulate
            eprintln!("Background prefetch: {}", pkg);
        });
    }

    /// Clear pending downloads
    pub fn clear_pending(&mut self) {
        self.pending_downloads.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markov_predictor() {
        let mut predictor = MarkovPredictor::new();

        // Observe patterns
        predictor.observe("react", "react-dom", "prop-types");
        predictor.observe("react", "react-dom", "prop-types");
        predictor.observe("react", "react-dom", "babel-runtime");

        // Predict
        let history = vec!["react".to_string(), "react-dom".to_string()];
        let predictions = predictor.predict_next(&history);

        assert!(!predictions.is_empty());
        assert_eq!(predictions[0].0, "prop-types"); // Most likely
        assert!(predictions[0].1 > 0.5); // >50% probability
    }

    #[test]
    fn test_predictor_save_load() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("predictor.bin");

        let mut predictor = MarkovPredictor::new();
        predictor.observe("pkg1", "pkg2", "pkg3");

        predictor.save(&path).unwrap();

        let loaded = MarkovPredictor::load(&path).unwrap();
        assert_eq!(loaded.transitions.len(), predictor.transitions.len());
    }

    #[test]
    fn test_predict_next_with_ricci() {
        let mut predictor = MarkovPredictor::new();
        predictor.observe("react", "react-dom", "pkg-a");
        predictor.observe("react", "react-dom", "pkg-bottleneck");

        let history = vec!["react".to_string(), "react-dom".to_string()];
        let mut curvatures = HashMap::new();
        curvatures.insert("pkg-a".to_string(), 2);
        curvatures.insert("pkg-bottleneck".to_string(), -15);

        let predictions = predictor.predict_next_with_ricci(&history, Some(&curvatures));
        assert!(!predictions.is_empty());
        // pkg-bottleneck should be boosted to first position
        assert_eq!(predictions[0].0, "pkg-bottleneck");
    }
}
