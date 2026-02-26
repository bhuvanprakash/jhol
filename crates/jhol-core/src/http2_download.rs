//! Fast HTTP/2 downloads using reqwest
//! Provides multiplexed downloads over a single HTTP/2 connection

use reqwest::Client;
use std::sync::Arc;

lazy_static::lazy_static! {
    /// Shared HTTP/2 client for multiplexed downloads
    static ref HTTP2_CLIENT: Arc<Client> = {
        let client = Client::builder()
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP/2 client");
        Arc::new(client)
    };
}

/// Download bytes using HTTP/2 client (multiplexed)
pub async fn download_http2(url: &str) -> Result<Vec<u8>, String> {
    let client = HTTP2_CLIENT.clone();
    
    let resp = client.get(url).send().await
        .map_err(|e| format!("HTTP/2 request failed: {}", e))?;
    
    let bytes = resp.bytes().await
        .map_err(|e| format!("Failed to read response: {}", e))?
        .to_vec();
    
    Ok(bytes)
}

/// Download and save to file using HTTP/2 client
pub async fn download_http2_to_file(url: &str, dest: &std::path::Path) -> Result<(), String> {
    let client = HTTP2_CLIENT.clone();
    
    let resp = client.get(url).send().await
        .map_err(|e| format!("HTTP/2 request failed: {}", e))?;
    
    let bytes = resp.bytes().await
        .map_err(|e| format!("Failed to read response: {}", e))?;
    
    std::fs::write(dest, &bytes)
        .map_err(|e| format!("Failed to write file: {}", e))?;
    
    Ok(())
}
