fn main() {
    let code = match rtk::run_from_args(std::env::args_os()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rtk: {:#}", e);
            1
        }
    };
    std::process::exit(code);
}
