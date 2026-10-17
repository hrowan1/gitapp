fn main() {
    build_recorder();
    tauri_build::build()
}

/// Build the Swift meeting recorder (macOS only, a SwiftPM package in `recorder/`) into `binaries/`,
/// where Tauri picks it up as a sidecar. SwiftPM rebuilds only what changed.
fn build_recorder() {
    println!("cargo:rerun-if-changed=recorder/Sources");
    println!("cargo:rerun-if-changed=recorder/Package.swift");
    println!("cargo:rerun-if-changed=recorder/Package.resolved");
    let target = std::env::var("TARGET").unwrap_or_default();
    // Lets the app find the helper in development, before Tauri bundles it next to the binary.
    println!("cargo:rustc-env=RECORDER_TRIPLE={target}");
    let out = format!("binaries/dev-inbox-recorder-{target}");
    // Gitignored, so a fresh checkout doesn't have it.
    std::fs::create_dir_all("binaries").ok();
    if !target.contains("apple-darwin") {
        // Other platforms: no recorder yet. Tauri still expects the sidecar file to exist.
        if !std::path::Path::new(&out).exists() {
            std::fs::write(&out, "").ok();
        }
        return;
    }
    let arch = if target.starts_with("aarch64") { "arm64" } else { "x86_64" };
    let status = std::process::Command::new("xcrun")
        .args(["swift", "build", "-c", "release", "--package-path", "recorder", "--arch", arch, "--product", "dev-inbox-recorder"])
        .status()
        .expect("Swift (Xcode command line tools) is needed to build the meeting recorder");
    assert!(status.success(), "building the meeting recorder failed");
    // Where SwiftPM put it (the layout differs between toolchains).
    let bin = std::process::Command::new("xcrun")
        .args(["swift", "build", "-c", "release", "--package-path", "recorder", "--arch", arch, "--show-bin-path"])
        .output()
        .expect("asking SwiftPM where the recorder is");
    let built = std::path::Path::new(String::from_utf8_lossy(&bin.stdout).trim()).join("dev-inbox-recorder");
    std::fs::copy(&built, &out).unwrap_or_else(|e| panic!("copying the recorder from {}: {e}", built.display()));
}
