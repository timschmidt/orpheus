//! Links pass 1 of the paged layout (link/paged-flat.ld); `cargo xtask
//! build` runs pass 2 (`xtask pagelink`) on the saved link.
fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-search={dir}/link");
    println!("cargo:rustc-link-arg-bins=-Tpaged-flat.ld");
    println!("cargo:rerun-if-changed=link");
}
