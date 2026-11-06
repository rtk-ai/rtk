// musl's built-in malloc is slow for the hook's allocation pattern; see Cargo.toml.
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    std::process::exit(rtk::entry());
}
