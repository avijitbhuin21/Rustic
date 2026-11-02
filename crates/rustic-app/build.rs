//! Embeds the product version (root `package.json`, which `scripts/bump-version.ps1`
//! keeps in sync with the desktop app) as `RUSTIC_APP_VERSION`, so the desktop app and
//! rustic-server report the same version to peers regardless of crate versions.

fn main() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let pkg = manifest.join("..").join("..").join("package.json");
    println!("cargo:rerun-if-changed={}", pkg.display());
    let version = std::fs::read_to_string(&pkg)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("version").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    println!("cargo:rustc-env=RUSTIC_APP_VERSION={version}");
}
