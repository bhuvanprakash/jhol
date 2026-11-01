fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=build.rs");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".into());
    println!();
    println!("  ╔═══════════════════════════════════════════════════════════════╗");
    println!("  ║                                                               ║");
    println!("  ║     ██╗  ██╗ ██████╗ ██╗  ██╗██╗   ██╗                     ║");
    println!("  ║     ██║  ██║██╔═══██╗██║ ██╔╝██║   ██║                     ║");
    println!("  ║     ███████║██║   ██║█████╔╝ ██║   ██║                     ║");
    println!("  ║     ██╔══██║██║   ██║██╔═██╗ ██║   ██║                     ║");
    println!("  ║     ██║  ██║╚██████╔╝██║  ██╗╚██████╔╝                     ║");
    println!("  ║     ╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═╝ ╚═════╝                      ║");
    println!("  ║                                                               ║");
    println!("  ║     Fast  Offline  Cache-First Package Manager              ║");
    println!("  ║     Powered by JAGR (Tropical + Sheaf + MaxSAT)             ║");
    println!("  ║                                                               ║");
    println!("  ╚═══════════════════════════════════════════════════════════════╝");
    println!();
    println!("  Compiling jhol v{} ...", version);
    println!();
}
