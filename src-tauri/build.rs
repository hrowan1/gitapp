fn main() {
    build_recorder();
    tauri_build::build()
}

/// Compile the Swift meeting recorder (macOS only) into `binaries/`, where Tauri picks it up as a
/// sidecar. Rebuilt only when its source changes.
fn build_recorder() {
    println!("cargo:rerun-if-changed=recorder/Recorder.swift");
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
        .args([
            "swiftc",
            "-O",
            "-parse-as-library",
            "-target",
            &format!("{arch}-apple-macos14.2"),
            "recorder/Recorder.swift",
            "-o",
            &out,
        ])
        .status()
        .expect("swiftc (Xcode command line tools) is needed to build the meeting recorder");
    assert!(status.success(), "building the meeting recorder failed");
}
