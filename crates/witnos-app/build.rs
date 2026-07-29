use std::path::Path;

fn main() {
    // tauri-build validates that every bundle resource exists at cargo-build
    // time. The bundled CLI is staged by scripts/install-app.sh; for plain
    // `cargo build/test` runs, stage the freshest built `witnos` bin if one
    // exists, else an empty placeholder (dev builds resolve the CLI from the
    // target dir at runtime anyway — see bundled_cli()).
    let staged = Path::new("binaries/witnos");
    if !staged.exists() {
        std::fs::create_dir_all("binaries").expect("create binaries staging dir");
        let built = ["../../target/release/witnos", "../../target/debug/witnos"]
            .into_iter()
            .map(Path::new)
            .find(|p| p.is_file());
        match built {
            Some(bin) => {
                std::fs::copy(bin, staged).expect("stage witnos bin");
            }
            None => {
                std::fs::write(staged, b"").expect("stage placeholder");
            }
        }
    }
    tauri_build::build()
}
