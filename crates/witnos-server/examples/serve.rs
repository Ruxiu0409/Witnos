//! Run the core headless (no GUI shell yet): binds an ephemeral port,
//! writes `$WITNOS_HOME/endpoint.json` (default `~/.witnos`), serves until
//! Ctrl-C, then removes armed markers (graceful stop).

use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home: PathBuf = match std::env::var("WITNOS_HOME") {
        Ok(h) => h.into(),
        Err(_) => {
            let home = std::env::var("HOME").expect("HOME is set");
            std::path::Path::new(&home).join(".witnos")
        }
    };
    let handle = witnos_server::start(&home).await?;
    println!(
        "witnos core listening on 127.0.0.1:{} (endpoint file: {})",
        handle.port,
        home.join("endpoint.json").display()
    );
    println!("Ctrl-C stops gracefully (removes armed markers).");
    tokio::signal::ctrl_c().await?;
    witnos_server::graceful_stop(&handle.state);
    println!("stopped; armed markers removed");
    Ok(())
}
