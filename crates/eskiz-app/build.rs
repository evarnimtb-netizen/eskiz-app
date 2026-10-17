//! Embeds the Eskiz icon and version resource into `eskiz.exe` when targeting Windows.
//!
//! The target is checked through `CARGO_CFG_TARGET_OS`, not `cfg!`, because build scripts
//! run on the host and a cross-build to Windows must still get the resource. For every
//! other target this script does nothing. When the host is not Windows and no resource compiler
//! (`llvm-rc`) is available, the resource is skipped with a `cargo:warning` instead of failing.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../packaging/icons/eskiz.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let icon = manifest_dir.join("../../packaging/icons/eskiz.ico");

    // The version fields (file and product version) come from CARGO_PKG_VERSION by default.
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(&icon.to_string_lossy());
    resource.set("ProductName", "Eskiz");
    resource.set("FileDescription", "Eskiz painting app");
    resource.set("InternalName", "eskiz");
    resource.set("OriginalFilename", "eskiz.exe");
    match resource.compile() {
        Ok(()) => {}
        // Cross-compiling to Windows from macOS or Linux (cargo-xwin, `tools/check_msvc.sh`):
        // winresource looks for `llvm-rc` on a non-Windows host and fails if LLVM is not on PATH.
        // Skip the resource there, with a visible warning, so the exe still builds for a compile
        // check. On a Windows host the resource is part of the product, so that still fails loudly.
        Err(err) if !cfg!(windows) => {
            println!(
                "cargo:warning=eskiz.exe will have no icon or version resource: {err} \
                 (install LLVM so `llvm-rc` is on PATH for cross builds to Windows)"
            );
        }
        Err(err) => panic!("failed to embed the Windows icon and version resource: {err}"),
    }
}
