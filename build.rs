fn main() {
    // uiAccess puts the menu above every always-on-top window; Windows only honours it
    // for a signed exe in a protected folder (install.ps1 signs it in Program Files).
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:level='asInvoker' uiAccess='true'");
    println!("cargo:rerun-if-changed=assets/HomeBar.ico");
    embed_resource::compile("assets/HomeBar.rc", embed_resource::NONE).manifest_required().unwrap();
}
