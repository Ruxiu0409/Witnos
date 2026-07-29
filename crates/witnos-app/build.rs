use std::path::Path;

/// What gets staged when no `witnos` bin has been built yet. It has to be
/// *something*: tauri-build validates every bundle resource exists at
/// cargo-build time, and `cargo test -p witnos-app` must work on a machine
/// that has never built the CLI. It must not be an empty file, which is what
/// it used to be — an empty file with the exec bit runs as an empty shell
/// script and **exits 0**, so a bundle carrying it looked healthy to every
/// check in the chain (install-app.sh's guard, `bundled_cli()`'s `is_file`)
/// and only failed as "the app opens with no terminals". Failing loudly on
/// the first exec is the whole point of the text below.
const PLACEHOLDER: &[u8] =
    b"#!/bin/sh\necho 'witnos: build placeholder, not the real CLI' >&2\nexit 1\n";

fn main() {
    // A real bundle stages the CLI from `beforeBuildCommand` (tauri.conf.json)
    // *before* cargo runs, because the bundler copies this path at bundle
    // time and may well skip re-running this script. So all that is left here
    // is the plain `cargo build/test -p witnos-app` case, where the runtime
    // resolves the CLI from the target dir anyway (see `bundled_cli()`).
    //
    // The rule is "whatever is staged must equal the best real bin available",
    // not "leave it alone if a file is there" — the old `if !staged.exists()`
    // froze the first answer forever, so one build made before the CLI existed
    // left every later bundle shipping a broken CLI. Comparing bytes rather
    // than recognising the placeholder also covers the stale case, and can't
    // be defeated by the placeholder text drifting by a space.
    let staged = Path::new("binaries/witnos");
    std::fs::create_dir_all("binaries").expect("create binaries staging dir");
    let built = ["../../target/release/witnos", "../../target/debug/witnos"]
        .into_iter()
        .map(Path::new)
        .find(|p| p.is_file());
    match built {
        // Release first: a bundle always builds it (beforeBuildCommand), so
        // debug only wins when it is the only real CLI on the machine.
        Some(bin) => {
            let same = match (std::fs::read(bin), std::fs::read(staged)) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            };
            if !same {
                std::fs::copy(bin, staged).expect("stage witnos bin");
            }
        }
        None => {
            if !staged.exists() {
                std::fs::write(staged, PLACEHOLDER).expect("stage placeholder");
                set_executable(staged);
            }
        }
    }
    tauri_build::build()
}

/// The placeholder only gets to report itself if it can be executed at all —
/// an unreadable resource fails as a permission error, which says nothing.
fn set_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect("mark placeholder executable");
    }
    #[cfg(not(unix))]
    let _ = path;
}
