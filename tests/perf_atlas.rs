//! Performance atlas: map-backed structures and hot-path microchecks.

use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

fn run_dashmap(workers: usize, keys: usize, rounds: usize) -> u128 {
    let map = Arc::new(DashMap::with_capacity(keys));
    for i in 0..keys {
        map.insert(format!("k{i}"), i as u64);
    }

    let start = Instant::now();
    let mut handles = Vec::new();
    for worker in 0..workers {
        let map = Arc::clone(&map);
        handles.push(thread::spawn(move || {
            let mut checksum = 0u64;
            for round in 0..rounds {
                let idx = (worker * rounds + round) % keys;
                let key = format!("k{idx}");
                if round % 8 == 0 {
                    map.insert(key, (round + worker) as u64);
                } else if let Some(value) = map.get(&key) {
                    checksum ^= *value;
                }
            }
            checksum
        }));
    }

    for handle in handles {
        let _ = handle.join().unwrap();
    }
    start.elapsed().as_millis()
}

fn run_rwlock(workers: usize, keys: usize, rounds: usize) -> u128 {
    let map = Arc::new(RwLock::new(HashMap::<String, u64>::with_capacity(keys)));
    {
        let mut guard = map.write();
        for i in 0..keys {
            guard.insert(format!("k{i}"), i as u64);
        }
    }

    let start = Instant::now();
    let mut handles = Vec::new();
    for worker in 0..workers {
        let map = Arc::clone(&map);
        handles.push(thread::spawn(move || {
            let mut checksum = 0u64;
            for round in 0..rounds {
                let idx = (worker * rounds + round) % keys;
                let key = format!("k{idx}");
                if round % 8 == 0 {
                    map.write().insert(key, (round + worker) as u64);
                } else if let Some(value) = map.read().get(&key) {
                    checksum ^= *value;
                }
            }
            checksum
        }));
    }

    for handle in handles {
        let _ = handle.join().unwrap();
    }
    start.elapsed().as_millis()
}

#[test]
#[ignore]
fn compare_dashmap_to_rwlock_under_contention() {
    let workers = std::thread::available_parallelism()
        .map(|n| n.get().max(4))
        .unwrap_or(4);
    let keys = 4_096;
    let rounds = 20_000;

    let dashmap_ms = run_dashmap(workers, keys, rounds);
    let rwlock_ms = run_rwlock(workers, keys, rounds);

    eprintln!(
        "map contention benchmark: workers={} keys={} rounds={} dashmap_ms={} rwlock_ms={}",
        workers, keys, rounds, dashmap_ms, rwlock_ms
    );
}
