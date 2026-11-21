//! Native fake for CLI routing tests: records the executable and argument vector
//! without requiring Node, a shell, a package registry, or installed JS runners.
use std::env;
use std::fs::OpenOptions;
use std::io::Write;

fn main() {
    let executable = env::current_exe().expect("current executable");
    let name = executable.file_stem().expect("executable name");
    let mut trace = OpenOptions::new()
        .create(true)
        .append(true)
        .open(env::var_os("RTK_TEST_TRACE").expect("trace path"))
        .expect("open trace");
    writeln!(trace, "{}", name.to_string_lossy()).expect("record executable");
    for arg in env::args().skip(1) {
        writeln!(trace, "{arg}").expect("record argument");
    }
    writeln!(trace, "END").expect("record invocation end");
    std::io::stdout()
        .write_all(env::var("RTK_TEST_STDOUT").unwrap_or_default().as_bytes())
        .expect("write stdout");
    std::io::stderr()
        .write_all(env::var("RTK_TEST_STDERR").unwrap_or_default().as_bytes())
        .expect("write stderr");
    std::process::exit(
        env::var("RTK_TEST_EXIT")
            .expect("exit code")
            .parse()
            .expect("numeric exit code"),
    );
}
