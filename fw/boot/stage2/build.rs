fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg-bins=-T{dir}/stage2.ld");
    println!("cargo:rerun-if-changed={dir}/stage2.ld");
}
