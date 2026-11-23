fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg-bins=-T{dir}/../../atj2157-hal/link/ram.ld");
    println!("cargo:rerun-if-changed={dir}/../../atj2157-hal/link/ram.ld");
}
