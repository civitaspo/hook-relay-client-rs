// The release version comes from `.release-version`, which the Release PR
// workflow writes; Cargo.toml's version is not bumped by releases.
fn main() {
    println!("cargo:rerun-if-changed=.release-version");
    let version = std::fs::read_to_string(".release-version")
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|_| "0.0.0-dev".to_string());
    println!("cargo:rustc-env=HOOK_RELAY_CLIENT_VERSION={version}");
}
