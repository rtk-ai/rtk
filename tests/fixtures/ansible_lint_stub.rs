use std::io::Write;
fn main() {
    if let Some(path) = std::env::var_os("ANSIBLE_LINT_TEST_LOG") {
        let mut log = std::fs::File::create(path).expect("argv log");
        for arg in std::env::args_os().skip(1) {
            log.write_all(arg.as_encoded_bytes()).expect("arg");
            log.write_all(b"\0").expect("boundary");
        }
    }
    let mode = std::env::var("ANSIBLE_LINT_TEST_MODE").unwrap_or_default();
    match mode.as_str() {
        "passthrough" => {
            std::io::stdout().write_all(b"\x1b[31m{\"items\": []}\x1b[0m").expect("stdout");
            std::io::stderr().write_all(b"native warning without newline").expect("stderr");
            std::process::exit(17);
        }
        "empty" => (),
        "clean" => eprintln!("Passed: 0 failure(s), 0 warning(s) in 1 files processed of 1 encountered."),
        "unknown" => {
            eprintln!("ERROR custom rule could not load\n  check configuration");
            std::process::exit(9);
        }
        _ => {
            for line in 2..=7 {
                println!("name[missing]: All tasks should be named.\nroles/example/tasks/main.yml:{line} Task/Handler: shell echo hello\n");
            }
            eprintln!("Failed: 6 failure(s), 0 warning(s) in 1 files processed of 1 encountered.");
            std::process::exit(2);
        }
    }
}
