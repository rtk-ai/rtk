//! cmake build/configure output — drown the progress spam, keep the diagnostics.

use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("cmake");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: cmake {}", args.join(" "));
    }

    runner::run_filtered_with_exit(
        cmd,
        "cmake",
        &args.join(" "),
        filter_cmake_with_exit,
        runner::RunOptions::with_tee("cmake"),
    )
}

pub(crate) fn filter_cmake(output: &str) -> String {
    filter_cmake_with_exit(output, 0)
}

fn filter_cmake_with_exit(output: &str, exit_code: i32) -> String {
    let mut objects = 0usize;
    let mut targets = 0usize;
    let mut saw_progress = false;
    let mut saw_error = false;
    let mut kept: Vec<&str> = Vec::new();

    for raw in output.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();

        // "[ 42%] Building CXX object ..." — one per translation unit. Tally, don't echo.
        if is_progress_pct(t) {
            saw_progress = true;
            if t.contains("] Built target ") {
                targets += 1;
            } else if t.contains(" Building ") && t.contains(" object ") {
                objects += 1;
            }
            continue;
        }

        // make's recursive Entering/Leaving-directory bookkeeping: pure noise.
        if t.starts_with("make[")
            && (t.contains("Entering directory")
                || t.contains("Leaving directory")
                || t.contains("Nothing to be done"))
        {
            continue;
        }
        if t.starts_with("Scanning dependencies of target") {
            continue;
        }
        // configure spends 200 lines poking the toolchain; keep the verdict, ditch the poking.
        if is_probe(t) {
            continue;
        }

        if is_error(t) {
            saw_error = true;
        }
        if !t.is_empty() {
            kept.push(line);
        }
    }

    let failed = exit_code != 0 || saw_error;
    let mut out = String::new();

    if failed {
        if exit_code != 0 {
            out.push_str(&format!("cmake: build failed (exit {exit_code})\n"));
        } else {
            out.push_str("cmake: build failed\n");
        }
    } else if saw_progress {
        out.push_str(&format!(
            "cmake: built {targets} target{}, {objects} object{}\n",
            plural(targets),
            plural(objects),
        ));
    }

    for line in &kept {
        out.push_str(line);
        out.push('\n');
    }

    let trimmed = out.trim_end();
    if trimmed.is_empty() {
        "cmake: nothing to do".to_string()
    } else {
        trimmed.to_string()
    }
}

// A leading "[<ws><digits>%]" marker — cmake/make progress, regardless of verb.
fn is_progress_pct(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('[') else {
        return false;
    };
    let Some(close) = rest.find("%]") else {
        return false;
    };
    let pct = rest[..close].trim();
    !pct.is_empty() && pct.bytes().all(|b| b.is_ascii_digit())
}

fn is_probe(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("-- ") else {
        return false;
    };
    rest.starts_with("Check")
        || rest.starts_with("Detecting")
        || rest.starts_with("Looking for")
        || rest.starts_with("Performing Test")
}

fn is_error(line: &str) -> bool {
    line.contains("error:")
        || line.contains("CMake Error")
        || line.contains("] Error ")
        || line.contains("***")
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD_OK: &str = r#"[  2%] Building CXX object src/CMakeFiles/engine.dir/alloc.cpp.o
[  5%] Building CXX object src/CMakeFiles/engine.dir/buffer.cpp.o
[  8%] Building CXX object src/CMakeFiles/engine.dir/cache.cpp.o
[ 11%] Building CXX object src/CMakeFiles/engine.dir/codec.cpp.o
[ 14%] Building CXX object src/CMakeFiles/engine.dir/config.cpp.o
src/config.cpp: In function 'int load(Ctx&)':
src/config.cpp:42:9: warning: unused variable 'tmp' [-Wunused-variable]
   42 |     int tmp = 0;
      |         ^~~
[ 17%] Building CXX object src/CMakeFiles/engine.dir/db.cpp.o
[ 20%] Building CXX object src/CMakeFiles/engine.dir/exec.cpp.o
[ 23%] Building CXX object src/CMakeFiles/engine.dir/hash.cpp.o
[ 26%] Building CXX object src/CMakeFiles/engine.dir/index.cpp.o
[ 29%] Building CXX object src/CMakeFiles/engine.dir/journal.cpp.o
[ 32%] Building CXX object src/CMakeFiles/engine.dir/lexer.cpp.o
Scanning dependencies of target engine
[ 47%] Linking CXX static library libengine.a
[ 50%] Built target engine
make[1]: Entering directory '/home/dev/proj/build'
[ 55%] Building CXX object app/CMakeFiles/shell.dir/main.cpp.o
[ 61%] Building CXX object app/CMakeFiles/shell.dir/repl.cpp.o
[ 88%] Building CXX object app/CMakeFiles/shell.dir/render.cpp.o
make[1]: Leaving directory '/home/dev/proj/build'
[ 94%] Linking CXX executable shell
[100%] Built target shell"#;

    #[test]
    fn collapses_progress_and_keeps_the_warning() {
        let out = filter_cmake(BUILD_OK);
        assert!(out.starts_with("cmake: built 2 targets, 14 objects"), "{out}");
        // the single warning is the whole point of reading build output — keep it verbatim
        assert!(out.contains("unused variable 'tmp'"));
        assert!(out.contains("src/config.cpp:42:9"));
        // ...and the noise is gone
        assert!(!out.contains("Building CXX object"));
        assert!(!out.contains("Scanning dependencies"));
        assert!(!out.contains("Entering directory"));
        assert!(!out.contains("Linking CXX"));

        let savings = 1.0 - (out.len() as f64 / BUILD_OK.len() as f64);
        assert!(savings >= 0.60, "expected >=60% savings, got {savings:.2}");
    }

    #[test]
    fn failure_preserves_diagnostics_drops_progress() {
        let input = r#"[ 20%] Building CXX object src/CMakeFiles/x.dir/main.cpp.o
src/main.cpp:10:5: error: 'foo' was not declared in this scope
   10 |     foo();
      |     ^~~
make[2]: *** [CMakeFiles/x.dir/build.make:76: main.cpp.o] Error 1
make[1]: *** [CMakeFiles/Makefile2:83: x] Error 2
make: *** [Makefile:91: all] Error 2"#;
        let out = filter_cmake_with_exit(input, 2);
        assert!(out.starts_with("cmake: build failed (exit 2)"));
        assert!(out.contains("error: 'foo' was not declared"));
        assert!(out.contains("main.cpp:10:5"));
        assert!(!out.contains("Building CXX object"));
    }

    #[test]
    fn configure_keeps_summary_drops_probe_spam() {
        let input = r#"-- The C compiler identification is GNU 11.4.0
-- Detecting C compiler ABI info
-- Detecting C compiler ABI info - done
-- Check for working C compiler: /usr/bin/cc - skipped
-- Looking for pthread.h
-- Looking for pthread.h - found
-- Performing Test HAVE_STD_FILESYSTEM
-- Performing Test HAVE_STD_FILESYSTEM - Success
-- Configuring done
-- Generating done
-- Build files have been written to: /home/dev/proj/build"#;
        let out = filter_cmake(input);
        assert!(out.contains("Configuring done"));
        assert!(out.contains("Build files have been written"));
        assert!(!out.contains("Detecting C compiler ABI"));
        assert!(!out.contains("Performing Test"));
        assert!(!out.contains("Looking for pthread"));
    }
}
