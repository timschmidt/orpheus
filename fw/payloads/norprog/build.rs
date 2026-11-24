fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg-bins=-T{dir}/payload.ld");
    println!("cargo:rerun-if-changed={dir}/payload.ld");
}
