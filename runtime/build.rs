fn main() {
    println!("cargo:rustc-env=APEIRON_VERSION={}", env!("CARGO_PKG_VERSION"));
}
