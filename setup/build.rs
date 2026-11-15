use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:level='asInvoker' uiAccess='false'");
    // version 6 common controls, for TaskDialog
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
         processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
    );
    println!("cargo:rerun-if-changed=setup.rc");
    println!("cargo:rerun-if-changed=../assets/HomeBar.ico");
    embed_resource::compile("setup.rc", embed_resource::NONE).manifest_required().unwrap();
    build_app();
}

/// Builds HomeBar.exe for the setup to carry. It gets its own target folder because this
/// build holds the lock on the main one.
fn build_app() {
    let workspace = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).parent().unwrap().to_path_buf();
    let target_dir = workspace.join("target").join("app");
    let profile = env::var("PROFILE").unwrap();
    for input in ["src", "assets", "build.rs", "Cargo.toml", "Cargo.lock"] {
        println!("cargo:rerun-if-changed={}", workspace.join(input).display());
    }

    let mut cargo = Command::new(env::var("CARGO").unwrap());
    cargo.current_dir(&workspace).args(["build", "--package", "homebar", "--locked", "--target-dir"]).arg(&target_dir);
    if profile == "release" {
        cargo.arg("--release");
    }
    // flags meant for this build script's own compilation, not for the app
    for var in ["CARGO_ENCODED_RUSTFLAGS", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"] {
        cargo.env_remove(var);
    }
    let status = cargo.status().expect("couldn't run cargo to build HomeBar.exe");
    assert!(status.success(), "building HomeBar.exe failed");

    let exe = target_dir.join(&profile).join("HomeBar.exe");
    println!("cargo:rustc-env=HOMEBAR_EXE={}", exe.display());
}
