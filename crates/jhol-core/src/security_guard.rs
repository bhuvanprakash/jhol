//! Supply-Chain Security & Typosquatting Guard
//!
//! Protects developers and AI agents from typosquatting attacks and malicious lookalike packages
//! by evaluating edit distances (Levenshtein) against top npm packages.

use std::collections::HashSet;

/// Top widely used npm packages prone to typosquatting attacks.
static POPULAR_PACKAGES: &[&str] = &[
    "react",
    "react-dom",
    "lodash",
    "express",
    "axios",
    "chalk",
    "tslib",
    "typescript",
    "commander",
    "moment",
    "vue",
    "next",
    "webpack",
    "eslint",
    "prettier",
    "cross-env",
    "dotenv",
    "glob",
    "debug",
    "fs-extra",
    "rxjs",
    "core-js",
    "semver",
    "mkdirp",
    "rimraf",
    "body-parser",
    "cors",
    "cookie-parser",
    "jsonwebtoken",
    "bcrypt",
    "mongoose",
    "pg",
    "mysql2",
    "redis",
    "winston",
    "morgan",
    "nodemon",
    "jest",
    "mocha",
    "chai",
    "supertest",
    "uuid",
    "nanoid",
    "dayjs",
    "date-fns",
    "yargs",
    "inquirer",
    "ora",
    "cheerio",
    "puppeteer",
    "tailwindcss",
    "postcss",
    "autoprefixer",
    "esbuild",
    "vite",
    "rollup",
    "babel-core",
    "@types/node",
    "@types/react",
    "styled-components",
    "classnames",
    "clsx",
    "zustand",
    "redux",
    "redux-thunk",
    "@reduxjs/toolkit",
    "zod",
    "yup",
    "joi",
    "formik",
    "react-hook-form",
    "query-string",
    "qs",
    "fast-glob",
    "chokidar",
    "minimist",
    "path-to-regexp",
    "mime-types",
];

/// Computes the Levenshtein distance between two strings.
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let len_a = a_chars.len();
    let len_b = b_chars.len();

    if len_a == 0 {
        return len_b;
    }
    if len_b == 0 {
        return len_a;
    }

    let mut prev_row: Vec<usize> = (0..=len_b).collect();
    let mut curr_row: Vec<usize> = vec![0; len_b + 1];

    for i in 0..len_a {
        curr_row[0] = i + 1;
        for j in 0..len_b {
            let cost = if a_chars[i] == b_chars[j] { 0 } else { 1 };
            curr_row[j + 1] = (prev_row[j + 1] + 1) // deletion
                .min(curr_row[j] + 1) // insertion
                .min(prev_row[j] + cost); // substitution
        }
        prev_row.copy_from_slice(&curr_row);
    }

    prev_row[len_b]
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TyposquatWarning {
    pub target_package: String,
    pub similar_popular_package: String,
    pub edit_distance: usize,
    pub warning_message: String,
}

/// Checks a package name against known popular packages and returns a warning if it looks like typosquatting.
pub fn check_typosquatting(name: &str) -> Option<TyposquatWarning> {
    let clean_name = name.trim().to_lowercase();
    let popular_set: HashSet<&'static str> = POPULAR_PACKAGES.iter().copied().collect();

    // Exact match is legitimate
    if popular_set.contains(clean_name.as_str()) {
        return None;
    }

    // Scoped packages like @types/node or @foo/bar
    let (_prefix, base_name) = if clean_name.starts_with('@') {
        if let Some(slash_idx) = clean_name.find('/') {
            (&clean_name[..slash_idx], &clean_name[slash_idx + 1..])
        } else {
            ("", clean_name.as_str())
        }
    } else {
        ("", clean_name.as_str())
    };

    let mut closest_match: Option<(&'static str, usize)> = None;

    for &popular in POPULAR_PACKAGES {
        let popular_base = if popular.starts_with('@') {
            popular.split('/').nth(1).unwrap_or(popular)
        } else {
            popular
        };

        // Check distance against the full name and base name
        let dist = levenshtein_distance(base_name, popular_base);

        // Edit distance of 1 or 2 is highly suspicious for strings of length >= 4
        if dist > 0 && dist <= 2 && popular_base.len() >= 4 {
            match closest_match {
                None => closest_match = Some((popular, dist)),
                Some((_, best_dist)) if dist < best_dist => closest_match = Some((popular, dist)),
                _ => {}
            }
        }
    }

    if let Some((popular, dist)) = closest_match {
        Some(TyposquatWarning {
            target_package: clean_name.clone(),
            similar_popular_package: popular.to_string(),
            edit_distance: dist,
            warning_message: format!(
                "⚠️ Supply-Chain Alert: \"{}\" is very similar to popular package \"{}\" (edit distance: {}). Verify package name.",
                clean_name, popular, dist
            ),
        })
    } else {
        None
    }
}

/// Batch check multiple packages and log warnings if any typosquatting suspects are found.
pub fn audit_packages_for_typosquatting(packages: &[String]) -> Vec<TyposquatWarning> {
    let mut warnings = Vec::new();
    for pkg in packages {
        if let Some(warn) = check_typosquatting(pkg) {
            eprintln!("{}", warn.warning_message);
            warnings.push(warn);
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levenshtein_distance() {
        assert_eq!(levenshtein_distance("react", "react"), 0);
        assert_eq!(levenshtein_distance("react", "reactt"), 1);
        assert_eq!(levenshtein_distance("cross-env", "cross-env-pkg"), 4);
        assert_eq!(levenshtein_distance("lodash", "loadash"), 1);
        assert_eq!(levenshtein_distance("express", "expres"), 1);
    }

    #[test]
    fn test_typosquat_detection() {
        // Exact popular packages should produce no warning
        assert!(check_typosquatting("react").is_none());
        assert!(check_typosquatting("lodash").is_none());
        assert!(check_typosquatting("express").is_none());

        // Typosquat candidates
        let warn1 = check_typosquatting("loadash").expect("loadash should trigger warning");
        assert_eq!(warn1.similar_popular_package, "lodash");
        assert_eq!(warn1.edit_distance, 1);

        let warn2 = check_typosquatting("expres").expect("expres should trigger warning");
        assert_eq!(warn2.similar_popular_package, "express");
        assert_eq!(warn2.edit_distance, 1);
    }
}
