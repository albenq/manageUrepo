use std::env;
use std::path::Path;

fn main() {
    // Re-run when the manifest changes so the embedded dependency list and
    // build time stay current.
    println!("cargo:rerun-if-changed=Cargo.toml");

    // Embedded about-window data, for every target OS:
    // - GIT_PM_DEPENDENCIES: runtime dependency crate names from the
    //   [dependencies] sections of Cargo.toml (lightweight line parsing,
    //   no TOML crate in the build graph);
    // - GIT_PM_BUILD_EPOCH: unix seconds of this build (formatted at
    //   runtime by `format_build_time_utc`).
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let manifest = std::fs::read_to_string(Path::new(&manifest_dir).join("Cargo.toml"))
        .expect("read Cargo.toml");
    let deps = collect_dependency_names(&manifest);
    println!("cargo:rustc-env=GIT_PM_DEPENDENCIES={}", deps.join(","));
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after 1970")
        .as_secs();
    println!("cargo:rustc-env=GIT_PM_BUILD_EPOCH={epoch}");

    // Only embed resources when compiling for Windows targets;
    // this is a no-op on every other target OS (cross-compile friendly).
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // Icon path is resolved relative to the project root (the manifest dir),
    // not the current working directory.
    let icon_path = Path::new(&manifest_dir).join("assets").join("icon.ico");

    let result = winresource::WindowsResource::new()
        .set_icon(icon_path.to_str().expect("icon path is valid UTF-8"))
        .compile();

    // A missing/broken resource toolchain must not break the build:
    // log a warning and continue without the embedded icon.
    if let Err(e) = result {
        println!("cargo:warning=failed to embed application icon: {e}");
    }
}

/// Extract runtime dependency crate names from Cargo.toml text: every
/// `name = ...` / `name = { ... }` line inside a `[...dependencies]`
/// section (root and per-target). Build-dependencies are excluded.
fn collect_dependency_names(manifest: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut in_deps = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_deps = trimmed.to_ascii_lowercase().contains("dependencies");
            continue;
        }
        if !in_deps || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(eq) = trimmed.find('=') else {
            continue;
        };
        let name = trimmed[..eq].trim().to_string();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}
