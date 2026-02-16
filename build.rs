fn main() {
    // Windows default stack is 1 MB, which overflows in debug builds
    // due to the large Commands enum (30+ Clap variants) and match routing.
    // Set 8 MB stack to match Unix defaults.
    #[cfg(target_os = "windows")]
    println!("cargo:rustc-link-arg=/STACK:8388608");
}
