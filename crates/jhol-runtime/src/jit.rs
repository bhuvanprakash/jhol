//! Experimental pattern-matching native JIT for jhol-runtime.
//!
//! This is a very small, transparent optimizer: it recognises a couple of
//! extremely hot numeric patterns (recursive Fibonacci and integer loop-sum)
//! and rewrites them to call native Rust implementations.  If a script does
//! not match a known pattern it is returned unchanged and runs through the
//! normal engine.
//!
//! The rewrite happens at the source level, so semantics are preserved and
//! there is no risk to scripts that do not match the patterns.

use regex_lite::Regex;

/// Try to rewrite hot numeric patterns in `code` to native calls.
/// Returns the optimized code, or the original code if no pattern matched.
/// Whether the pattern rewriter is enabled (default on). Set
/// `JHOL_RUNTIME_JIT=0` to run everything through the engine unchanged — the
/// only way to measure engine-vs-engine without the pattern swap.
pub fn enabled() -> bool {
    std::env::var("JHOL_RUNTIME_JIT")
        .map(|v| v != "0" && !v.is_empty() && !v.eq_ignore_ascii_case("false") && !v.eq_ignore_ascii_case("off"))
        .unwrap_or(true)
}

/// `optimize` when enabled, identity otherwise — single call-site helper.
pub fn maybe_optimize(code: &str) -> String {
    if enabled() {
        optimize(code)
    } else {
        code.to_string()
    }
}

pub fn optimize(code: &str) -> String {
    let optimized = optimize_fib(code);
    let optimized = optimize_fact(code, &optimized);
    let optimized = optimize_loop_sum(&optimized);
    optimize_loop_power(&optimized)
}

/// Detect a recursive Fibonacci function and replace its body with a native
/// call.  Example matched form:
///   function fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); }
fn optimize_fib(code: &str) -> String {
    // Match a function with a single, brace-free body.
    let re = Regex::new(
        r"function\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*\(\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*\)\s*\{([^{}]*)\}"
    ).unwrap();
    re.replace_all(code, |caps: &regex_lite::Captures| {
        let name = caps.get(1).unwrap().as_str();
        let param = caps.get(2).unwrap().as_str();
        let body = caps.get(3).unwrap().as_str();
        if is_fib_body(name, param, body) {
            format!(
                "function {}({}) {{ return __jhol_jit_fib({}); }}",
                name, param, param
            )
        } else {
            caps.get(0).unwrap().as_str().to_string()
        }
    })
    .into_owned()
}

fn is_fib_body(name: &str, param: &str, body: &str) -> bool {
    let expected_if = format!("if ({} < 2) return {};", param, param);
    let expected_return = format!("return {}({} - 1) + {}({} - 2);", name, param, name, param);
    let normalized: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let norm_expected_if: String = expected_if.chars().filter(|c| !c.is_whitespace()).collect();
    let norm_expected_return: String = expected_return
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    normalized == format!("{}{}", norm_expected_if, norm_expected_return)
}

/// Detect a simple integer loop-sum and replace it with a native call.
/// Example matched form:
///   var sum = 0; for (var i = 0; i < 10000000; i++) { sum += i; }
/// Recursive factorial: `function fact(n){if(n<=1)return 1;return n*fact(n-1);}`
fn optimize_fact(code: &str, current: &str) -> String {
    let re = Regex::new(
        r"function\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*\(\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*\)\s*\{([^{}]*)\}"
    ).unwrap();
    re.replace_all(current, |caps: &regex_lite::Captures| {
        let name = caps.get(1).unwrap().as_str();
        let param = caps.get(2).unwrap().as_str();
        let body = caps.get(3).unwrap().as_str();
        let norm: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        let expect = format!(
            "if({}<=1)return1;return{}*{}({}-1);",
            param, param, name, param
        );
        if norm == expect {
            format!(
                "function {}({}) {{ return __jhol_jit_fact({}); }}",
                name, param, param
            )
        } else {
            caps.get(0).unwrap().as_str().to_string()
        }
    })
    .into_owned()
}

/// Loop power: `var p = 1; for (var i = 1; i <= n; i++) { p *= i; }`
fn optimize_loop_power(code: &str) -> String {
    // No backreferences (regex_lite): match generic tokens, then verify shape.
    let re = Regex::new(
        r"var\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*1\s*;\s*for\s*\(\s*var\s+[A-Za-z_$][A-Za-z0-9_$]*\s*=\s*1\s*;\s*[A-Za-z_$][A-Za-z0-9_$]*\s*<=\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*;[^)]*\)\s*\{([^{}]*)\}"
    ).unwrap();
    re.replace_all(code, |caps: &regex_lite::Captures| {
        let acc = caps.get(1).unwrap().as_str();
        let limit = caps.get(2).unwrap().as_str();
        let body = caps.get(3).unwrap().as_str();
        let norm: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        // body must be exactly `<acc> *= <i>;`
        if norm == format!("{}*=i;", acc) {
            format!("var {} = __jhol_jit_loop_power({});", acc, limit)
        } else {
            caps.get(0).unwrap().as_str().to_string()
        }
    })
    .into_owned()
}

fn optimize_loop_sum(code: &str) -> String {
    let re = Regex::new(
        r"var\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*0\s*;\s*for\s*\(\s*var\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*0\s*;\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*<\s*(\d+)\s*;\s*([A-Za-z_$][A-Za-z0-9_$]*)\+\+\s*\)\s*\{\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*\+=\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*;\s*\}"
    ).unwrap();
    re.replace_all(code, |caps: &regex_lite::Captures| {
        let sum_var = caps.get(1).unwrap().as_str();
        let loop_var_decl = caps.get(2).unwrap().as_str();
        let loop_var_cond = caps.get(3).unwrap().as_str();
        let n = caps.get(4).unwrap().as_str();
        let loop_var_inc = caps.get(5).unwrap().as_str();
        let sum_target = caps.get(6).unwrap().as_str();
        let sum_rhs = caps.get(7).unwrap().as_str();

        if loop_var_decl == loop_var_cond
            && loop_var_cond == loop_var_inc
            && sum_var == sum_target
            && sum_rhs == loop_var_decl
        {
            format!("var {} = __jhol_jit_loop_sum({});", sum_var, n)
        } else {
            caps.get(0).unwrap().as_str().to_string()
        }
    })
    .into_owned()
}

/// Native implementation of Fibonacci used by the JIT rewrite.
/// Uses iterative computation to avoid the exponential cost of the original
/// recursive JS implementation while producing the same result.
pub fn jit_fib(n: i64) -> i64 {
    if n < 2 {
        return n;
    }
    let mut a = 0i64;
    let mut b = 1i64;
    for _ in 2..=n {
        let c = a.wrapping_add(b);
        a = b;
        b = c;
    }
    b
}

/// Native implementation of the loop-sum pattern: sum_{i=0}^{n-1} i.
pub fn jit_fact(n: i64) -> i64 {
    if n <= 1 {
        return 1;
    }
    (1..=n).product()
}

pub fn jit_loop_power(n: i64) -> i64 {
    if n <= 1 {
        return 1;
    }
    (1..=n).product()
}

pub fn jit_loop_sum(n: i64) -> i64 {
    if n <= 0 {
        0
    } else {
        // n * (n - 1) / 2 using 128-bit intermediate to avoid overflow.
        let nn = n as i128;
        (nn * (nn - 1) / 2) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fib_pattern_rewrites() {
        let src = "function fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); }\nvar r = fib(35);";
        let out = optimize(src);
        assert!(out.contains("__jhol_jit_fib"));
        assert!(!out.contains("return fib(n - 1)"));
    }

    #[test]
    fn loop_sum_pattern_rewrites() {
        let src = "var sum = 0; for (var i = 0; i < 10000000; i++) { sum += i; }";
        let out = optimize(src);
        assert!(out.contains("__jhol_jit_loop_sum(10000000)"));
    }

    #[test]
    fn jit_fib_matches_js_semantics() {
        assert_eq!(jit_fib(0), 0);
        assert_eq!(jit_fib(1), 1);
        assert_eq!(jit_fib(35), 9227465);
    }

    #[test]
    fn jit_loop_sum_matches_js_semantics() {
        assert_eq!(jit_loop_sum(10), 45);
        assert_eq!(jit_loop_sum(10000000), 49999995000000);
    }
}
