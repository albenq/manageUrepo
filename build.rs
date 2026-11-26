use std::env;
use std::path::Path;

fn main() {
    // Only embed resources when compiling for Windows targets;
    // this is a no-op on every other target OS (cross-compile friendly).
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // Icon path is resolved relative to the project root (the manifest dir),
    // not the current working directory.
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
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
