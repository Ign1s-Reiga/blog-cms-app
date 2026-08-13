fn main() {
    embed_test_harness_manifest();
    tauri_build::build()
}

/// Give `cargo test` binaries the Common-Controls v6 manifest they need to start
/// on Windows.
///
/// `rustc-link-arg-tests` applies to test targets only, so the app binary keeps
/// the manifest tauri-build embeds and never sees two. See
/// `manifests/test-harness.manifest` for why the manifest is needed at all.
fn embed_test_harness_manifest() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }

    // `/MANIFESTINPUT` needs an absolute path; the linker's working directory is
    // not this crate's.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("manifests")
        .join("test-harness.manifest");

    println!("cargo:rerun-if-changed=manifests/test-harness.manifest");
    println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}", manifest.display());
}
