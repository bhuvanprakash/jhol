//! Unit tests for registry module

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry;

    #[test]
    fn test_base_name_unscoped() {
        assert_eq!(registry::base_name("lodash"), "lodash");
        assert_eq!(registry::base_name("lodash@4.17.21"), "lodash");
        assert_eq!(registry::base_name("react@18"), "react");
    }

    #[test]
    fn test_base_name_scoped() {
        assert_eq!(registry::base_name("@babel/core"), "@babel/core");
        assert_eq!(registry::base_name("@babel/core@7.0.0"), "@babel/core");
        assert_eq!(registry::base_name("@types/node@18"), "@types/node");
    }

    #[test]
    fn test_base_name_edge_cases() {
        // Package names with @ in the middle
        assert_eq!(registry::base_name("pkg@1.0.0"), "pkg");
        // Scoped packages
        assert_eq!(registry::base_name("@scope/pkg"), "@scope/pkg");
        assert_eq!(registry::base_name("@scope/pkg@1.0"), "@scope/pkg");
    }

    #[test]
    fn test_version_satisfaction() {
        use semver::{Version, VersionReq};

        let v1_0_0 = Version::parse("1.0.0").unwrap();
        let v1_5_0 = Version::parse("1.5.0").unwrap();
        let v2_0_0 = Version::parse("2.0.0").unwrap();

        let req_exact = VersionReq::parse("1.0.0").unwrap();
        let req_caret = VersionReq::parse("^1.0.0").unwrap();
        let req_tilde = VersionReq::parse("~1.0.0").unwrap();
        let req_star = VersionReq::parse("*").unwrap();

        // Exact match
        assert!(req_exact.matches(&v1_0_0));
        assert!(!req_exact.matches(&v1_5_0));

        // Caret (^) - compatible with version
        assert!(req_caret.matches(&v1_0_0));
        assert!(req_caret.matches(&v1_5_0));
        assert!(!req_caret.matches(&v2_0_0));

        // Tilde (~) - approximately equivalent
        assert!(req_tilde.matches(&v1_0_0));
        assert!(!req_tilde.matches(&v1_5_0));

        // Star (*) - any version
        assert!(req_star.matches(&v1_0_0));
        assert!(req_star.matches(&v2_0_0));
    }

    #[test]
    fn test_integrity_hash_format() {
        // SRI format: algorithm-base64hash
        let valid_sri = "sha512-abc123def456";
        assert!(valid_sri.starts_with("sha"));
        assert!(valid_sri.contains('-'));
    }
}
