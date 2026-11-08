fn main() {
    println!("cargo:rerun-if-env-changed=LIBCXX_DIR");
    if let Some(dir) = std::env::var_os("LIBCXX_DIR") {
        println!("cargo:rustc-link-search=native={}", dir.to_string_lossy());
    }
}
