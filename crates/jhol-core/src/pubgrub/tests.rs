//! Tests for PubGrub solver

use super::*;
use version_set::{PackedVersion, VersionRange, VersionSet};
use std::collections::HashMap;

#[test]
fn test_simple_dependency_resolution() {
    let mut solver = PubGrubSolver::new("root".to_string());
    
    // Root requires pkg-a ^1.0.0
    let mut requirements = HashMap::new();
    requirements.insert(
        "pkg-a".to_string(),
        VersionSet::from_range(VersionRange {
            min: PackedVersion::parse("1.0.0").unwrap(),
            max: PackedVersion::parse("1.255.255").unwrap(),
            min_inclusive: true,
            max_inclusive: true,
        }),
    );
    solver.add_root_requirements(requirements);
    
    solver.set_available_versions_from_strings("pkg-a", vec![
        "1.0.0".to_string(),
        "1.1.0".to_string(),
        "1.2.0".to_string(),
    ]);
    
    let result = solver.solve().unwrap();
    assert!(result.contains_key("pkg-a"));
    
    // Should choose highest compatible version
    let version = result.get("pkg-a").unwrap();
    assert_eq!(*version, PackedVersion::parse("1.2.0").unwrap());
}

#[test]
fn test_multiple_packages() {
    let mut solver = PubGrubSolver::new("root".to_string());
    
    let mut requirements = HashMap::new();
    requirements.insert(
        "react".to_string(),
        VersionSet::from_range(VersionRange {
            min: PackedVersion::parse("18.0.0").unwrap(),
            max: PackedVersion::parse("18.255.255").unwrap(),
            min_inclusive: true,
            max_inclusive: true,
        }),
    );
    requirements.insert(
        "react-dom".to_string(),
        VersionSet::from_range(VersionRange {
            min: PackedVersion::parse("18.0.0").unwrap(),
            max: PackedVersion::parse("18.255.255").unwrap(),
            min_inclusive: true,
            max_inclusive: true,
        }),
    );
    solver.add_root_requirements(requirements);
    
    solver.set_available_versions_from_strings("react", vec![
        "18.0.0".to_string(),
        "18.2.0".to_string(),
    ]);
    solver.set_available_versions_from_strings("react-dom", vec![
        "18.0.0".to_string(),
        "18.2.0".to_string(),
    ]);
    
    let result = solver.solve().unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.contains_key("react"));
    assert!(result.contains_key("react-dom"));
}

#[test]
fn test_no_solution() {
    let mut solver = PubGrubSolver::new("root".to_string());
    
    // Require both ^1.0.0 and ^2.0.0 - impossible
    let mut requirements = HashMap::new();
    requirements.insert(
        "pkg".to_string(),
        VersionSet::from_range(VersionRange {
            min: PackedVersion::parse("1.0.0").unwrap(),
            max: PackedVersion::parse("1.255.255").unwrap(),
            min_inclusive: true,
            max_inclusive: true,
        }),
    );
    requirements.insert(
        "pkg".to_string(),
        VersionSet::from_range(VersionRange {
            min: PackedVersion::parse("2.0.0").unwrap(),
            max: PackedVersion::parse("2.255.255").unwrap(),
            min_inclusive: true,
            max_inclusive: true,
        }),
    );
    solver.add_root_requirements(requirements);
    
    solver.set_available_versions_from_strings("pkg", vec![
        "1.0.0".to_string(),
        "2.0.0".to_string(),
    ]);
    
    let result = solver.solve();
    assert!(matches!(result, Err(PubGrubError::NoSolution(_))));
}

#[test]
fn test_version_selection() {
    let mut solver = PubGrubSolver::new("root".to_string());
    
    let mut requirements = HashMap::new();
    requirements.insert(
        "pkg".to_string(),
        VersionSet::from_range(VersionRange {
            min: PackedVersion::parse("1.0.0").unwrap(),
            max: PackedVersion::parse("3.0.0").unwrap(),
            min_inclusive: true,
            max_inclusive: true,
        }),
    );
    solver.add_root_requirements(requirements);
    
    // Available versions
    solver.set_available_versions_from_strings("pkg", vec![
        "1.0.0".to_string(),
        "2.0.0".to_string(),
        "2.5.0".to_string(),
        "3.0.0".to_string(),
    ]);
    
    let result = solver.solve().unwrap();
    let version = result.get("pkg").unwrap();
    
    // Should choose highest compatible version (3.0.0)
    assert_eq!(*version, PackedVersion::parse("3.0.0").unwrap());
}

#[test]
fn test_solver_stats() {
    let mut solver = PubGrubSolver::new("root".to_string());
    
    let mut requirements = HashMap::new();
    requirements.insert(
        "pkg".to_string(),
        VersionSet::any(),
    );
    solver.add_root_requirements(requirements);
    
    solver.set_available_versions_from_strings("pkg", vec![
        "1.0.0".to_string(),
    ]);
    
    let result = solver.solve();
    assert!(result.is_ok());
    
    let stats = solver.stats();
    assert!(stats.decisions >= 1);
}
