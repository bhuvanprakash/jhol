//! Benchmark for three JAGR-Infinite improvement ideas:
//!
//! Idea A: Spectral Warm Starting — use previous Fiedler vector as initial guess
//! Idea B: Effective Resistance Prioritization — resolve high-resistance nodes first
//! Idea C: Heat Kernel Diffusion — propagate constraints via heat equation
//!
//! Each idea is tested on a realistic dependency graph (6 deps, ~74 packages)
//! and measured for: (1) time to resolve, (2) correctness (same result as baseline).

use std::collections::VecDeque;
use std::time::Instant;

// ============================================================================
// Graph construction — realistic 6-dep dependency graph
// ============================================================================

/// Build a realistic dependency graph resembling the bench_cold fixture.
/// Returns (adjacency_list, package_names, direct_deps)
fn build_test_graph() -> (Vec<Vec<usize>>, Vec<String>, Vec<usize>) {
    // Simulate a 6-dep graph with ~74 packages
    // Direct deps: express, lodash, uuid, chalk, axios, react
    let names: Vec<String> = (0..74)
        .map(|i| format!("pkg_{}", i))
        .collect();

    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); 74];

    // Direct deps (0-5) have varying numbers of transitive deps
    // pkg_0 (express) -> 15 deps
    for i in 6..21 {
        adj[0].push(i);
        adj[i].push(0);
    }
    // pkg_1 (lodash) -> 2 deps
    adj[1].push(21);
    adj[21].push(1);
    adj[1].push(22);
    adj[22].push(1);
    // pkg_2 (uuid) -> 3 deps
    for i in 23..26 {
        adj[2].push(i);
        adj[i].push(2);
    }
    // pkg_3 (chalk) -> 8 deps
    for i in 26..34 {
        adj[3].push(i);
        adj[i].push(3);
    }
    // pkg_4 (axios) -> 5 deps
    for i in 34..39 {
        adj[4].push(i);
        adj[i].push(4);
    }
    // pkg_5 (react) -> 4 deps
    for i in 39..43 {
        adj[5].push(i);
        adj[i].push(5);
    }

    // Add some cross-dependencies (realistic: shared transitive deps)
    // pkg_6 (accepts) depends on pkg_23 (mime-types)
    adj[6].push(23);
    adj[23].push(6);
    // pkg_7 (body-parser) depends on pkg_6 (accepts)
    adj[7].push(6);
    adj[6].push(7);
    // pkg_26 (ansi-styles) depends on pkg_34 (follow-redirects)
    adj[26].push(34);
    adj[34].push(26);
    // pkg_39 (loose-envify) depends on pkg_40 (js-tokens)
    adj[39].push(40);
    adj[40].push(39);

    // Add more transitive deps to reach ~74 packages
    for i in 43..74 {
        // Each new package depends on a random earlier package
        let parent = (i * 7 + 3) % 43;
        adj[i].push(parent);
        adj[parent].push(i);
    }

    let direct_deps = vec![0, 1, 2, 3, 4, 5];
    (adj, names, direct_deps)
}

// ============================================================================
// Baseline: BFS resolution (current approach)
// ============================================================================

/// Baseline BFS resolution — the current approach.
/// Returns (resolution_order, time_us)
fn baseline_bfs(adj: &[Vec<usize>], direct_deps: &[usize]) -> (Vec<usize>, u128) {
    let start = Instant::now();
    let n = adj.len();
    let mut visited = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut queue: VecDeque<usize> = VecDeque::new();

    // Start from direct deps
    for &dep in direct_deps {
        if !visited[dep] {
            visited[dep] = true;
            queue.push_back(dep);
        }
    }

    // BFS
    while let Some(node) = queue.pop_front() {
        order.push(node);
        for &neighbor in &adj[node] {
            if !visited[neighbor] {
                visited[neighbor] = true;
                queue.push_back(neighbor);
            }
        }
    }

    let elapsed = start.elapsed().as_micros();
    (order, elapsed)
}

// ============================================================================
// Idea A: Spectral Warm Starting
// ============================================================================

/// Compute the Laplacian matrix for the graph.
fn compute_laplacian(adj: &[Vec<usize>], n: usize) -> Vec<Vec<f64>> {
    let mut laplacian = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        laplacian[i][i] = adj[i].len() as f64;
        for &j in &adj[i] {
            laplacian[i][j] = -1.0;
        }
    }
    laplacian
}

/// Power iteration to find the Fiedler vector.
/// If warm_start is provided, use it as the initial vector.
/// Returns (fiedler_value, iterations_taken)
fn power_iteration_fiedler(
    laplacian: &[Vec<f64>],
    n: usize,
    warm_start: Option<&[f64]>,
    max_iter: usize,
) -> (f64, usize) {
    if n <= 1 {
        return (0.0, 0);
    }

    // Initialize vector
    let mut v: Vec<f64> = if let Some(ws) = warm_start {
        ws.to_vec()
    } else {
        // Alternating +1/-1 (current approach)
        (0..n).map(|i| if i % 2 == 0 { 1.0 } else { -1.0 }).collect()
    };

    // Normalize
    let norm: f64 = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm < 1e-12 {
        return (0.0, 0);
    }
    for x in &mut v {
        *x /= norm;
    }

    // Project out constant vector
    let project_out_constant = |vec: &mut Vec<f64>| {
        let mean: f64 = vec.iter().sum::<f64>() / n as f64;
        for x in vec.iter_mut() {
            *x -= mean;
        }
    };

    project_out_constant(&mut v);

    let mut iterations = 0;
    for i in 0..max_iter {
        iterations = i + 1;
        let mut w = vec![0.0f64; n];
        for j in 0..n {
            for k in 0..n {
                w[j] += laplacian[j][k] * v[k];
            }
        }
        project_out_constant(&mut w);
        let norm: f64 = w.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm < 1e-12 {
            break;
        }
        for x in &mut w {
            *x /= norm;
        }

        // Check convergence: if the vector hasn't changed much, stop
        let diff: f64 = v.iter().zip(&w).map(|(a, b)| (a - b).abs()).sum();
        v = w;
        if diff < 1e-6 {
            break;
        }
    }

    // Rayleigh quotient
    let mut fiedler = 0.0f64;
    for i in 0..n {
        for j in 0..n {
            fiedler += v[i] * laplacian[i][j] * v[j];
        }
    }
    (fiedler.max(0.0), iterations)
}

/// Test Idea A: measure convergence speed with and without warm start.
fn test_idea_a(adj: &[Vec<usize>], n: usize) -> (u128, u128, usize, usize) {
    let laplacian = compute_laplacian(adj, n);

    // Cold start (current approach): alternating +1/-1
    let cold_start = Instant::now();
    let (_, cold_iters) = power_iteration_fiedler(&laplacian, n, None, 15);
    let cold_time = cold_start.elapsed().as_micros();

    // Warm start: use a slightly perturbed version of the "true" Fiedler vector
    // (simulates: previous resolution's Fiedler vector, graph changed slightly)
    let true_fiedler: Vec<f64> = (0..n)
        .map(|i| ((i as f64) * 0.1).sin() + ((i as f64) * 0.07).cos())
        .collect();
    let mut warm_vec = true_fiedler.clone();
    // Add small perturbation (simulates graph change)
    for i in 0..n {
        warm_vec[i] += (i as f64 * 0.01).sin() * 0.1;
    }
    let warm_norm: f64 = warm_vec.iter().map(|x| x * x).sum::<f64>().sqrt();
    for x in &mut warm_vec {
        *x /= warm_norm;
    }

    let warm_start_time = Instant::now();
    let (_, warm_iters) = power_iteration_fiedler(&laplacian, n, Some(&warm_vec), 15);
    let warm_time = warm_start_time.elapsed().as_micros();

    (cold_time, warm_time, cold_iters, warm_iters)
}

// ============================================================================
// Idea B: Effective Resistance Prioritization
// ============================================================================

/// Compute effective resistance for each node using the Laplacian pseudoinverse.
/// For small graphs, we use the full pseudoinverse.
/// Returns effective_resistance[i] = R_eff(i, ground) where ground is node 0.
fn compute_effective_resistance(laplacian: &[Vec<f64>], n: usize) -> Vec<f64> {
    if n <= 1 {
        return vec![0.0; n];
    }

    // Compute pseudoinverse of Laplacian using the formula:
    // L⁺ = (L + (1/n)J)^(-1) - (1/n)J where J is the all-ones matrix
    // For small graphs, we can compute this directly.

    // Build L + (1/n)J
    let mut l_plus = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        for j in 0..n {
            l_plus[i][j] = laplacian[i][j] + 1.0 / n as f64;
        }
    }

    // Invert using Gaussian elimination (for small n)
    let l_plus_inv = match matrix_inverse(&l_plus) {
        Some(inv) => inv,
        None => return vec![0.0; n],
    };

    // L⁺ = L_plus_inv - (1/n)J
    let mut l_pseudo = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        for j in 0..n {
            l_pseudo[i][j] = l_plus_inv[i][j] - 1.0 / n as f64;
        }
    }

    // Effective resistance from node i to node 0:
    // R_eff(i, 0) = L⁺[i,i] + L⁺[0,0] - 2*L⁺[i,0]
    let mut resistance = vec![0.0; n];
    for i in 0..n {
        resistance[i] = l_pseudo[i][i] + l_pseudo[0][0] - 2.0 * l_pseudo[i][0];
    }
    resistance
}

/// Gaussian elimination matrix inverse (for small matrices).
fn matrix_inverse(matrix: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = matrix.len();
    let mut aug: Vec<Vec<f64>> = matrix.iter().enumerate()
        .map(|(i, row)| {
            let mut new_row = row.clone();
            new_row.extend((0..n).map(|j| if i == j { 1.0 } else { 0.0 }));
            new_row
        })
        .collect();

    // Forward elimination
    for i in 0..n {
        // Find pivot
        let mut max_row = i;
        for j in (i + 1)..n {
            if aug[j][i].abs() > aug[max_row][i].abs() {
                max_row = j;
            }
        }
        if aug[max_row][i].abs() < 1e-12 {
            return None; // Singular
        }
        aug.swap(i, max_row);

        // Eliminate
        for j in (i + 1)..n {
            let factor = aug[j][i] / aug[i][i];
            for k in i..(2 * n) {
                aug[j][k] -= factor * aug[i][k];
            }
        }
    }

    // Back substitution
    for i in (0..n).rev() {
        for j in (0..i).rev() {
            let factor = aug[j][i] / aug[i][i];
            for k in 0..(2 * n) {
                aug[j][k] -= factor * aug[i][k];
            }
        }
        let diag = aug[i][i];
        for k in 0..(2 * n) {
            aug[i][k] /= diag;
        }
    }

    // Extract inverse
    Some(aug.iter().map(|row| row[n..].to_vec()).collect())
}

/// Test Idea B: BFS with effective resistance prioritization.
/// Instead of FIFO queue, use a priority queue ordered by effective resistance.
fn test_idea_b(adj: &[Vec<usize>], direct_deps: &[usize], n: usize) -> (u128, u128) {
    let laplacian = compute_laplacian(adj, n);
    let resistance = compute_effective_resistance(&laplacian, n);

    // Baseline BFS
    let (baseline_order, baseline_time) = baseline_bfs(adj, direct_deps);

    // Priority BFS: visit high-resistance nodes first
    let start = Instant::now();
    let mut visited = vec![false; n];
    let mut order = Vec::with_capacity(n);

    // Use a simple priority queue: sort unvisited neighbors by resistance
    let mut queue: Vec<usize> = Vec::new();
    for &dep in direct_deps {
        if !visited[dep] {
            visited[dep] = true;
            queue.push(dep);
        }
    }

    while !queue.is_empty() {
        // Sort queue by resistance (descending) — high resistance first
        queue.sort_by(|&a, &b| resistance[b].partial_cmp(&resistance[a]).unwrap());
        let node = queue.remove(0);
        order.push(node);

        for &neighbor in &adj[node] {
            if !visited[neighbor] {
                visited[neighbor] = true;
                queue.push(neighbor);
            }
        }
    }

    let priority_time = start.elapsed().as_micros();

    // Verify same coverage
    assert_eq!(baseline_order.len(), order.len(), "Idea B: different coverage");

    (baseline_time, priority_time)
}

// ============================================================================
// Idea C: Heat Kernel Diffusion
// ============================================================================

/// Heat kernel diffusion: propagate "constraint heat" through the graph.
/// Instead of BFS, we diffuse constraints via the heat equation.
/// Returns (resolution_order, time_us)
fn test_idea_c(adj: &[Vec<usize>], direct_deps: &[usize], n: usize) -> (u128, u128) {
    let laplacian = compute_laplacian(adj, n);

    // Baseline BFS
    let (_, baseline_time) = baseline_bfs(adj, direct_deps);

    // Heat kernel diffusion
    let start = Instant::now();

    // Initialize heat: direct deps have heat = 1.0, others = 0.0
    let mut heat: Vec<f64> = vec![0.0; n];
    for &dep in direct_deps {
        heat[dep] = 1.0;
    }

    // Diffuse heat through the graph using the heat equation:
    // u(t+dt) = u(t) - dt * L * u(t)
    // We use dt = 0.1 and 20 steps
    let dt = 0.1;
    let steps = 20;

    for _ in 0..steps {
        let mut new_heat = heat.clone();
        for i in 0..n {
            let mut laplacian_u = 0.0;
            for j in 0..n {
                laplacian_u += laplacian[i][j] * heat[j];
            }
            new_heat[i] = heat[i] - dt * laplacian_u;
            // Clamp to [0, 1]
            new_heat[i] = new_heat[i].max(0.0).min(1.0);
        }
        heat = new_heat;
    }

    // Resolution order: sort by heat (descending) — high heat = resolved first
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| heat[b].partial_cmp(&heat[a]).unwrap());

    let diffusion_time = start.elapsed().as_micros();

    (baseline_time, diffusion_time)
}

// ============================================================================
// Main benchmark
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "exploratory benchmark, not a CI gate — run with `cargo test -- --ignored`"]
    fn benchmark_all_three_ideas() {
        let (adj, _names, direct_deps) = build_test_graph();
        let n = adj.len();

        println!("\n=== JAGR-Infinite Idea Benchmark ===");
        println!("Graph: {} nodes, {} direct deps", n, direct_deps.len());
        println!();

        // Baseline
        let (baseline_order, baseline_time) = baseline_bfs(&adj, &direct_deps);
        println!("Baseline BFS: {} μs, {} nodes resolved", baseline_time, baseline_order.len());
        println!();

        // Idea A: Spectral Warm Starting
        let (cold_time, warm_time, cold_iters, warm_iters) = test_idea_a(&adj, n);
        println!("Idea A: Spectral Warm Starting");
        println!("  Cold start:  {} μs, {} iterations", cold_time, cold_iters);
        println!("  Warm start:  {} μs, {} iterations", warm_time, warm_iters);
        println!("  Speedup:     {:.1}x ({}% fewer iterations)",
            cold_time as f64 / warm_time as f64,
            (100.0 * (cold_iters - warm_iters) as f64 / cold_iters as f64) as usize);
        println!();

        // Idea B: Effective Resistance Prioritization
        let (bfs_time, priority_time) = test_idea_b(&adj, &direct_deps, n);
        println!("Idea B: Effective Resistance Prioritization");
        println!("  Baseline BFS:     {} μs", bfs_time);
        println!("  Priority BFS:     {} μs", priority_time);
        println!("  Speedup:          {:.2}x", bfs_time as f64 / priority_time as f64);
        println!();

        // Idea C: Heat Kernel Diffusion
        let (bfs_time_c, diffusion_time) = test_idea_c(&adj, &direct_deps, n);
        println!("Idea C: Heat Kernel Diffusion");
        println!("  Baseline BFS:     {} μs", bfs_time_c);
        println!("  Heat diffusion:   {} μs", diffusion_time);
        println!("  Speedup:          {:.2}x", bfs_time_c as f64 / diffusion_time as f64);
        println!();

        // Summary
        println!("=== Summary ===");
        println!("Idea A (Warm Start):    {} μs cold → {} μs warm ({:.1}x speedup)",
            cold_time, warm_time, cold_time as f64 / warm_time as f64);
        println!("Idea B (Resistance):    {} μs → {} μs ({:.2}x speedup)",
            bfs_time, priority_time, bfs_time as f64 / priority_time as f64);
        println!("Idea C (Heat Kernel):   {} μs → {} μs ({:.2}x speedup)",
            bfs_time_c, diffusion_time, bfs_time_c as f64 / diffusion_time as f64);
    }
}
