//! Unit tests for SAT resolver (JAGR-1)

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sat_resolver::{solve_exact, PackageDomain, PackageVersion, SolveInput};
    use std::collections::HashMap;

    #[test]
    fn test_simple_dependency() {
        let mut domains: HashMap<String, PackageDomain> = HashMap::new();
        
        // Create a simple package domain
        let mut lodash_domain = PackageDomain::default();
        let mut versions = HashMap::new();
        versions.insert("dependencies".to_string(), HashMap::new());
        versions.insert("optional_dependencies".to_string(), HashMap::new());
        versions.insert("peer_dependencies".to_string(), HashMap::new());
        versions.insert("optional_peers".to_string(), std::collections::HashSet::new());
        
        let v4_17_21 = PackageVersion {
            version: "4.17.21".to_string(),
            dependencies: HashMap::new(),
            optional_dependencies: HashMap::new(),
            peer_dependencies: HashMap::new(),
            optional_peers: std::collections::HashSet::new(),
        };
        
        lodash_domain.versions.insert("4.17.21".to_string(), v4_17_21);
        domains.insert("lodash".to_string(), lodash_domain);

        let mut input = SolveInput::default();
        input.root_requirements.insert("lodash".to_string(), "^4.0.0".to_string());

        let result = solve_exact(&input, &domains);
        assert!(result.is_ok());
        let assignment = result.unwrap().assignment;
        assert_eq!(assignment.get("lodash"), Some(&"4.17.21".to_string()));
    }

    #[test]
    fn test_no_solution() {
        let mut domains: HashMap<String, PackageDomain> = HashMap::new();
        
        // Create a package with only one version
        let mut pkg_domain = PackageDomain::default();
        let v1_0_0 = PackageVersion {
            version: "1.0.0".to_string(),
            dependencies: HashMap::new(),
            optional_dependencies: HashMap::new(),
            peer_dependencies: HashMap::new(),
            optional_peers: std::collections::HashSet::new(),
        };
        pkg_domain.versions.insert("1.0.0".to_string(), v1_0_0);
        domains.insert("pkg".to_string(), pkg_domain);

        let mut input = SolveInput::default();
        // Require a version that doesn't exist
        input.root_requirements.insert("pkg".to_string(), "^2.0.0".to_string());

        let result = solve_exact(&input, &domains);
        assert!(result.is_err());
    }

    #[test]
    fn test_transitive_dependencies() {
        let mut domains: HashMap<String, PackageDomain> = HashMap::new();
        
        // Package A depends on B
        let mut a_deps = HashMap::new();
        a_deps.insert("B".to_string(), "^1.0.0".to_string());
        
        let a_v1 = PackageVersion {
            version: "1.0.0".to_string(),
            dependencies: a_deps,
            optional_dependencies: HashMap::new(),
            peer_dependencies: HashMap::new(),
            optional_peers: std::collections::HashSet::new(),
        };
        
        let mut a_domain = PackageDomain::default();
        a_domain.versions.insert("1.0.0".to_string(), a_v1);
        domains.insert("A".to_string(), a_domain);

        // Package B
        let b_v1 = PackageVersion {
            version: "1.0.0".to_string(),
            dependencies: HashMap::new(),
            optional_dependencies: HashMap::new(),
            peer_dependencies: HashMap::new(),
            optional_peers: std::collections::HashSet::new(),
        };
        
        let mut b_domain = PackageDomain::default();
        b_domain.versions.insert("1.0.0".to_string(), b_v1);
        domains.insert("B".to_string(), b_domain);

        let mut input = SolveInput::default();
        input.root_requirements.insert("A".to_string(), "^1.0.0".to_string());

        let result = solve_exact(&input, &domains);
        assert!(result.is_ok());
        let assignment = result.unwrap().assignment;
        assert!(assignment.contains_key("A"));
        assert!(assignment.contains_key("B"));
    }

    #[test]
    fn test_version_ordering() {
        use crate::sat_resolver::cmp_semver_desc;
        
        assert!(cmp_semver_desc("2.0.0", "1.0.0").is_gt());
        assert!(cmp_semver_desc("1.5.0", "1.0.0").is_gt());
        assert!(cmp_semver_desc("1.0.0", "1.0.0").is_eq());
        assert!(cmp_semver_desc("1.0.0", "2.0.0").is_lt());
    }
}
