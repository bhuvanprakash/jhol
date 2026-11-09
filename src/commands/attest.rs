//! `jhol attest` — the verifier's own verdict, written down.

use crate::flag;
use clap::ArgMatches;

pub fn run(sub_m: &ArgMatches) -> Result<(), String> {
    if flag(&sub_m, "verify") {
        let Some(att) = jhol_core::attest::Attestation::load(std::path::Path::new(".")) else {
            return Err("no attestation found; run `jhol attest` first".into());
        };
        match att.verify(std::path::Path::new(".")) {
            Ok(()) => {
                if !flag(&sub_m, "json") {
                    println!("attestation verified ({} pinned packages)", att.packages.len());
                }
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    }
    let att = jhol_core::attest::Attestation::build(std::path::Path::new("."))
        .map_err(|e| format!("attest: {e}"))?;
    att.save(std::path::Path::new(".")).map_err(|e| e.to_string())?;
    if flag(&sub_m, "json") {
        println!("{}", serde_json::to_string_pretty(&att).unwrap_or_default());
    } else {
        println!(
            "attested {} packages and {} models at {}",
            att.packages.len(),
            att.models.len(),
            jhol_core::attest::Attestation::path(std::path::Path::new(".")).display()
        );
    }
    return Ok(());
}
