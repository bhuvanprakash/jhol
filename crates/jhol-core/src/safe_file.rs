//! Safe file operations with error handling

pub fn log_warning(msg: &str) {
    eprintln!("WARNING: {}", msg);
}

pub fn log_error(msg: &str) {
    eprintln!("ERROR: {}", msg);
}

pub fn ensure_dir(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn atomic_write_json<T: serde::Serialize>(
    path: &std::path::Path,
    data: &T,
) -> Result<(), String> {
    let content = serde_json::to_string(data).map_err(|e| e.to_string())?;
    std::fs::write(path, content).map_err(|e| e.to_string())?;
    Ok(())
}
