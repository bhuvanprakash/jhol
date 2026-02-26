//! JAGR-3: Adaptive Heuristic (replaces static VSIDS)
//! 
//! Uses dual exponential moving averages to adaptively control search.
//! Inspired by AutoModSAT (2025) but simplified - no LLM required.

use std::collections::HashMap;

/// Adaptive heuristic for variable selection
pub struct AdaptiveHeuristic {
    // Dual exponential moving averages for LBD (Literal Block Distance)
    fast_lbd_avg: f64,  // Short-term (90% historical weight)
    slow_lbd_avg: f64,  // Long-term (99% historical weight)
    
    // Activity scores for variables
    activity: HashMap<String, f64>,
    
    // Decay parameters
    decay: f64,
    decay_period: usize,
    decay_counter: usize,
    
    // Search tracking
    decision_level: usize,
    conflicts: usize,
    
    // Adaptive thresholds (tuned empirically)
    restart_threshold: f64,
    activity_scale: f64,
}

impl AdaptiveHeuristic {
    pub fn new() -> Self {
        Self {
            fast_lbd_avg: 1.0,
            slow_lbd_avg: 1.0,
            activity: HashMap::new(),
            decay: 1.0,
            decay_period: 100,
            decay_counter: 0,
            decision_level: 0,
            conflicts: 0,
            restart_threshold: 1.2,  // AutoModSAT-inspired
            activity_scale: 1.0,
        }
    }
    
    /// Update heuristic with new conflict information
    pub fn on_conflict(&mut self, lbd: f64, conflicting_vars: &[String]) {
        self.conflicts += 1;
        self.decision_level += 1;
        
        // Update dual EMAs (AutoModSAT innovation)
        self.fast_lbd_avg = 0.9 * self.fast_lbd_avg + 0.1 * lbd;
        self.slow_lbd_avg = 0.99 * self.slow_lbd_avg + 0.01 * lbd;
        
        // Bump activity for conflicting variables
        // Decision-level scaling improves search focus
        let increment = self.activity_scale * (1.0 + 0.1 * (self.decision_level as f64));
        for var in conflicting_vars {
            let entry = self.activity.entry(var.clone()).or_insert(0.0);
            *entry = (*entry + increment).min(1e100);  // Prevent overflow
        }
        
        // Periodic decay (prevents activity explosion)
        self.decay_counter += 1;
        if self.decay_counter >= self.decay_period {
            self.decay *= 1.05;
            if self.decay > 1e100 {
                // Rescale all activities to prevent overflow
                for val in self.activity.values_mut() {
                    *val *= 1e-100;
                }
                self.decay *= 1e-100;
            }
            self.decay_counter = 0;
        }
    }
    
    /// Decide whether to restart search based on search quality
    pub fn should_restart(&self) -> bool {
        let ratio = self.fast_lbd_avg / self.slow_lbd_avg;
        
        // AutoModSAT-inspired adaptive logic:
        // - ratio > 1.2: search quality degrading → restart
        // - ratio > 1.0: search stable → continue
        // - ratio <= 1.0: search improving → continue
        ratio > self.restart_threshold
    }
    
    /// Select next variable to branch on (highest activity)
    pub fn select_variable(&self, candidates: &[String]) -> Option<String> {
        candidates
            .iter()
            .max_by(|a, b| {
                let activity_a = self.activity.get(*a).unwrap_or(&0.0);
                let activity_b = self.activity.get(*b).unwrap_or(&0.0);
                activity_a.partial_cmp(activity_b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
    }
    
    /// Get current search quality metric (for debugging)
    pub fn search_quality(&self) -> f64 {
        self.slow_lbd_avg / self.fast_lbd_avg
    }
    
    /// Reset heuristic (for restarts)
    pub fn reset(&mut self) {
        self.decision_level = 0;
        // Keep activity scores (they're valuable across restarts)
        // Keep EMAs (they track search behavior)
    }
}

impl Default for AdaptiveHeuristic {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adaptive_heuristic_basic() {
        let mut heuristic = AdaptiveHeuristic::new();
        
        // Simulate some conflicts
        for i in 0..10 {
            heuristic.on_conflict(5.0 + (i as f64), &["var1".to_string(), "var2".to_string()]);
        }
        
        // Should have built up some activity
        assert!(heuristic.activity.get("var1").unwrap() > &0.0);
        assert!(heuristic.activity.get("var2").unwrap() > &0.0);
    }

    #[test]
    fn test_should_restart() {
        let mut heuristic = AdaptiveHeuristic::new();
        
        // Initially should not restart
        assert!(!heuristic.should_restart());
        
        // Simulate degrading search (increasing LBD)
        for i in 0..50 {
            heuristic.on_conflict(10.0 + (i as f64), &["var".to_string()]);
        }
        
        // Should eventually want to restart
        assert!(heuristic.should_restart());
    }

    #[test]
    fn test_select_variable() {
        let mut heuristic = AdaptiveHeuristic::new();
        
        // Build up different activity levels
        heuristic.on_conflict(5.0, &["low".to_string()]);
        for _ in 0..10 {
            heuristic.on_conflict(5.0, &["high".to_string()]);
        }
        
        let candidates = vec!["low".to_string(), "high".to_string()];
        let selected = heuristic.select_variable(&candidates).unwrap();
        
        // Should select variable with highest activity
        assert_eq!(selected, "high");
    }
}
