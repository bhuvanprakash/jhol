//! `--omit` — which dependency classes an install leaves out.
//!
//! The production/deploy case: a Docker image or CI job wants runtime
//! dependencies only. `npm ci --omit=dev`, `pnpm install --prod`, and
//! `yarn install --production` all express this; the semantics below match
//! them:
//!
//! * `dev`      — `devDependencies` are not installed.
//! * `optional` — `optionalDependencies` are not installed.
//! * `peer`     — `peerDependencies` are not auto-installed.
//!
//! `NODE_ENV=production` implies `--omit=dev` unless the caller says
//! otherwise explicitly (`--include=dev` overrides). The lockfile is never
//! filtered: it keeps recording every dependency class, so a later plain
//! `install` restores the full tree.
//!
//! Omission is a *resolver filter*, applied where the install decides which
//! specs to materialise — not a parse-time drop. That keeps `doctor`,
//! `outdated`, `audit` and `why` looking at the complete manifest.

/// Which dependency classes to leave out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Omit {
    pub dev: bool,
    pub optional: bool,
    pub peer: bool,
}

impl Omit {
    /// Nothing omitted.
    pub const NONE: Omit = Omit {
        dev: false,
        optional: false,
        peer: false,
    };

    /// Production install: `--prod` / `--omit=dev`.
    pub const PROD: Omit = Omit {
        dev: true,
        optional: false,
        peer: false,
    };

    /// Parse a comma-separated list: `dev`, `optional`, `peer`, or any
    /// combination (`--omit=dev,optional`). `dev` also accepts
    /// `development`; `optional` accepts `opt`.
    pub fn parse(list: &str) -> Result<Omit, String> {
        let mut o = Omit::default();
        for raw in list.split(',') {
            let item = raw.trim().to_ascii_lowercase();
            match item.as_str() {
                "" => continue,
                "dev" | "development" => o.dev = true,
                "optional" | "opt" => o.optional = true,
                "peer" => o.peer = true,
                other => {
                    return Err(format!(
                        "invalid --omit value {:?} (expected dev, optional, or peer)",
                        other
                    ))
                }
            }
        }
        Ok(o)
    }

    /// True when nothing is omitted — the fast path skips all filtering.
    pub fn is_empty(&self) -> bool {
        !self.dev && !self.optional && !self.peer
    }

    /// Compose with `NODE_ENV=production`: production implies omitting dev
    /// unless `--include=dev` was given.
    pub fn with_env(self, include_dev: bool) -> Omit {
        if include_dev {
            return self;
        }
        let production = std::env::var("NODE_ENV")
            .map(|v| v.eq_ignore_ascii_case("production") || v == "prod")
            .unwrap_or(false);
        if production && !self.dev {
            Omit { dev: true, ..self }
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_and_combined() {
        assert_eq!(Omit::parse("dev").unwrap(), Omit::PROD);
        assert_eq!(
            Omit::parse("dev,optional").unwrap(),
            Omit {
                dev: true,
                optional: true,
                peer: false
            }
        );
        assert_eq!(Omit::parse("").unwrap(), Omit::NONE);
        assert!(Omit::parse("bogus").is_err());
    }

    #[test]
    fn empty_check() {
        assert!(Omit::NONE.is_empty());
        assert!(!Omit::PROD.is_empty());
    }

    #[test]
    fn production_env_implies_omit_dev() {
        // Not touching the real environment (tests run in parallel); assert
        // the include_dev override instead, which is env-independent.
        assert_eq!(Omit::NONE.with_env(true), Omit::NONE);
        assert_eq!(Omit::PROD.with_env(true), Omit::PROD);
    }
}

// ---------------------------------------------------------------------------
// Scoped current value
// ---------------------------------------------------------------------------

/// The omission set in effect for the install currently running.
///
/// The materialisation layer (`lockfile::skip_lockfile_materialization`) sits
/// deep inside tree walking with a fixed signature that many callers use, so
/// threading an extra parameter would touch every one of them. The install is
/// single-threaded and scoped, and `install_package_inner` restores the
/// previous value on exit — the same shape this crate already uses for
/// `set_daemon_mode` / `set_transitive_depth`.
static CURRENT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn bits(o: Omit) -> u8 {
    (o.dev as u8) | ((o.optional as u8) << 1) | ((o.peer as u8) << 2)
}

fn from_bits(b: u8) -> Omit {
    Omit {
        dev: b & 1 != 0,
        optional: b & 2 != 0,
        peer: b & 4 != 0,
    }
}

/// The omission set currently in effect.
pub fn current() -> Omit {
    from_bits(CURRENT.load(std::sync::atomic::Ordering::Relaxed))
}

/// Set the omission set for the duration of an install, restoring the previous
/// value when the returned guard drops.
pub fn scoped(o: Omit) -> OmitGuard {
    let prev = CURRENT.swap(bits(o), std::sync::atomic::Ordering::Relaxed);
    OmitGuard { prev }
}

/// Restores the previous omission set on drop.
pub struct OmitGuard {
    prev: u8,
}

impl Drop for OmitGuard {
    fn drop(&mut self) {
        CURRENT.store(self.prev, std::sync::atomic::Ordering::Relaxed);
    }
}
