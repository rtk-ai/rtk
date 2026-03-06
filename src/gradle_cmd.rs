use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

/// Run any gradle command with filtered output
pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let gradle_bin = detect_gradle_binary();

    let mut cmd = Command::new(&gradle_bin);
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} {}", gradle_bin, args.join(" "));
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run {}. Is Gradle installed?", gradle_bin))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{}\n{}", stdout, stderr)
    };

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    let filtered = filter_gradle_output(&raw);

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "gradle", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("{} {}", gradle_bin, args.join(" ")),
        &format!("rtk gradle {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

/// Prefer ./gradlew if present, otherwise fall back to gradle
fn detect_gradle_binary() -> String {
    if std::path::Path::new("./gradlew").exists() {
        "./gradlew".to_string()
    } else {
        "gradle".to_string()
    }
}

/// Filter gradle output to reduce token consumption
pub fn filter_gradle_output(output: &str) -> String {
    if output.is_empty() {
        return String::new();
    }

    let mut result = Vec::new();
    let mut in_failed_test = false;
    let mut in_error_block = false;
    let mut in_try_block = false;
    let mut in_welcome_banner = false;
    let dep_depth_limit = 2; // show top-level + 1 level of deps
    let mut in_dep_tree = false;
    let mut seen_notes: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut last_was_blank = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip empty lines (collapse consecutive)
        if trimmed.is_empty() {
            if !last_was_blank && !result.is_empty() {
                result.push(String::new());
                last_was_blank = true;
            }
            continue;
        }
        last_was_blank = false;

        // === WELCOME BANNER (Gradle download/update) ===
        if trimmed.starts_with("Welcome to Gradle") {
            in_welcome_banner = true;
            continue;
        }
        if in_welcome_banner {
            if trimmed.starts_with("Starting a Gradle Daemon")
                || trimmed.starts_with("> Task")
                || trimmed.starts_with("> Configure")
                || trimmed.starts_with("BUILD")
            {
                in_welcome_banner = false;
                // fall through to process this line
            } else {
                continue;
            }
        }

        // === DOWNLOAD PROGRESS ===
        if trimmed.starts_with("Downloading https://")
            || trimmed.starts_with("......")
            || trimmed.starts_with("Starting a Gradle Daemon")
        {
            continue;
        }

        // === --info VERBOSE NOISE ===
        if trimmed.starts_with("Initialized native services")
            || trimmed.starts_with("Initialized jansi")
            || trimmed.starts_with("The client will now receive")
            || trimmed.starts_with("Starting Build")
            || trimmed.contains("daemon log file")
            || trimmed.starts_with("Starting ") && trimmed.contains("build in daemon")
            || trimmed.starts_with("Using ") && trimmed.contains("worker leases")
            || trimmed.starts_with("Now considering")
            || trimmed.starts_with("Watching the file system")
            || trimmed.starts_with("File system watching")
            || trimmed.starts_with("Settings evaluated")
            || trimmed.starts_with("Projects loaded")
            || trimmed.starts_with("Included projects")
            || trimmed.starts_with("Evaluating root project")
            || trimmed.starts_with("Evaluating project")
            || trimmed.starts_with("All projects evaluated")
            || trimmed.starts_with("Task name matched")
            || trimmed.starts_with("Selected primary task")
            || trimmed.starts_with("Resolving global dependency")
            || trimmed.starts_with("Caching disabled")
            || trimmed.starts_with("Skipping task")
        {
            continue;
        }

        // === CONFIGURE PROJECT (noise) ===
        if trimmed.starts_with("> Configure project") {
            continue;
        }
        if trimmed.contains("is an incubating feature") {
            continue;
        }

        // === * Try: block — skip all suggestion lines ===
        if trimmed == "* Try:" {
            in_try_block = true;
            continue;
        }
        if in_try_block {
            if trimmed.starts_with('>') || trimmed.starts_with("* ") {
                continue;
            }
            in_try_block = false;
            // fall through — non-suggestion line ends the block
        }

        // === TASK LINES ===
        if trimmed.starts_with("> Task ") {
            // Keep FAILED tasks
            if trimmed.ends_with("FAILED") {
                in_error_block = true;
                result.push(line.to_string());
                continue;
            }
            // Skip UP-TO-DATE, NO-SOURCE, SKIPPED, and successful tasks
            if trimmed.ends_with("UP-TO-DATE")
                || trimmed.ends_with("NO-SOURCE")
                || trimmed.ends_with("SKIPPED")
                || trimmed.ends_with("FROM-CACHE")
            {
                continue;
            }
            // For running tasks (no suffix), skip unless they produce output
            // We'll keep them only if they're followed by meaningful output
            continue;
        }

        // === TEST RESULTS ===
        // Keep FAILED test lines + their output
        if trimmed.contains(" FAILED") && !trimmed.starts_with("> Task") {
            in_failed_test = true;
            result.push(line.to_string());
            continue;
        }
        // Skip PASSED test lines
        if trimmed.contains(" PASSED") && !trimmed.starts_with("BUILD") {
            in_failed_test = false;
            continue;
        }
        // Keep stack trace / error output after a failed test
        if in_failed_test {
            if trimmed.starts_with("at ") || trimmed.starts_with("Caused by:") {
                // Truncate deep stack traces — keep first 3 frames
                let indent_level = line.len() - line.trim_start().len();
                if indent_level > 0 {
                    result.push(line.to_string());
                }
                continue;
            }
            if trimmed.starts_with("> Task")
                || trimmed.starts_with("BUILD")
                || trimmed.contains("tests completed")
            {
                in_failed_test = false;
                // fall through to process this line
            } else {
                result.push(line.to_string());
                continue;
            }
        }

        // === ERROR BLOCK (compilation errors after FAILED task) ===
        if in_error_block {
            if trimmed.starts_with("> Task") || trimmed.starts_with("BUILD") {
                in_error_block = false;
                // fall through to process this line
            } else {
                result.push(line.to_string());
                continue;
            }
        }

        // === DEPRECATION / NOTE DEDUP ===
        if trimmed.starts_with("Note: ") {
            // Deduplicate identical notes
            let note_key = if trimmed.contains("Recompile with") {
                "Recompile-note".to_string()
            } else {
                trimmed.to_string()
            };
            if seen_notes.contains(&note_key) {
                continue;
            }
            seen_notes.insert(note_key);
            result.push(line.to_string());
            continue;
        }

        // === DEPENDENCY TREE ===
        if trimmed.starts_with("----") && trimmed.ends_with("----") {
            // Header/separator lines in dependency output
            continue;
        }
        if trimmed.starts_with("Root project '") {
            continue;
        }
        if trimmed.ends_with(" - Compile classpath for source set 'main'.")
            || trimmed.ends_with(" - Runtime classpath of source set 'main'.")
            || trimmed.ends_with(" - Classpath for compiling the main sources.")
        {
            in_dep_tree = true;
            result.push(line.to_string());
            continue;
        }
        if in_dep_tree {
            if trimmed.starts_with("BUILD") {
                in_dep_tree = false;
                // fall through to BUILD summary handler
            } else {
                // Count tree depth by pipe/space prefix
                let depth = count_dep_depth(trimmed);
                if depth <= dep_depth_limit {
                    let clean = trimmed.replace(" (*)", "");
                    result.push(clean);
                }
                continue;
            }
        }

        // === [ant:*] OUTPUT (Etendo smartbuild etc.) — always keep ===
        if trimmed.starts_with("[ant:") {
            result.push(line.to_string());
            continue;
        }

        // === BUILD SUMMARY — always keep ===
        if trimmed.starts_with("BUILD SUCCESSFUL") || trimmed.starts_with("BUILD FAILED") {
            result.push(line.to_string());
            continue;
        }

        // === FAILURE header ===
        if trimmed.starts_with("FAILURE:") {
            result.push(line.to_string());
            continue;
        }

        // === ACTIONABLE TASKS summary ===
        if trimmed.contains("actionable task") {
            result.push(line.to_string());
            continue;
        }

        // === TEST SUMMARY (N tests completed, N failed) ===
        if trimmed.contains("tests completed") || trimmed.contains("test completed") {
            result.push(line.to_string());
            continue;
        }

        // === What went wrong ===
        if trimmed.starts_with("* What went wrong:") {
            result.push(line.to_string());
            continue;
        }
        if trimmed.starts_with("Execution failed for task")
            || trimmed.starts_with("> Compilation failed")
            || trimmed.starts_with("> There were failing tests")
        {
            result.push(line.to_string());
            continue;
        }

        // Default: skip unrecognized lines (most are noise in Gradle)
        // But keep lines that look like compiler/runtime errors
        if trimmed.contains("error:") || trimmed.contains("Error:") || trimmed.contains("Exception")
        {
            result.push(line.to_string());
            continue;
        }

        // Keep lines with file:line references (compiler errors)
        if trimmed.contains(".java:") || trimmed.contains(".kt:") || trimmed.contains(".groovy:") {
            result.push(line.to_string());
            continue;
        }

        // Keep caret lines (error pointer)
        if trimmed == "^"
            || trimmed.contains("symbol:")
            || trimmed.contains("location:")
            || trimmed.contains("required:")
            || trimmed.contains("found:")
        {
            result.push(line.to_string());
            continue;
        }

        // Keep error count lines
        if trimmed.ends_with("error")
            || trimmed.ends_with("errors")
            || trimmed.ends_with("warning")
            || trimmed.ends_with("warnings")
        {
            result.push(line.to_string());
            continue;
        }

        // Everything else is noise — skip
    }

    // Clean up leading and trailing blank lines
    while result.first().map_or(false, |l| l.is_empty()) {
        result.remove(0);
    }
    while result.last().map_or(false, |l| l.is_empty()) {
        result.pop();
    }

    result.join("\n")
}

/// Count dependency tree depth from prefix characters (|, +---, \---)
fn count_dep_depth(line: &str) -> usize {
    let prefix: String = line.chars().take_while(|c| !c.is_alphanumeric()).collect();
    prefix.matches('|').count()
        + if prefix.contains('+') || prefix.contains('\\') {
            1
        } else {
            0
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    const BUILD_SUCCESS: &str = "\
> Configure project :
Build cache is an incubating feature.

> Task :compileJava UP-TO-DATE
> Task :processResources NO-SOURCE
> Task :classes UP-TO-DATE
> Task :compileTestJava UP-TO-DATE
> Task :processTestResources NO-SOURCE
> Task :testClasses UP-TO-DATE
> Task :jar UP-TO-DATE
> Task :assemble UP-TO-DATE
> Task :spotbugsMain UP-TO-DATE
> Task :checkstyleMain UP-TO-DATE
> Task :check UP-TO-DATE
> Task :build UP-TO-DATE

BUILD SUCCESSFUL in 2s
7 actionable tasks: 7 up-to-date";

    const BUILD_FAILED: &str = "\
> Task :compileJava
> Task :processResources NO-SOURCE
> Task :classes

> Task :compileTestJava FAILED
/src/test/java/com/example/UserServiceTest.java:15: error: cannot find symbol
    UserService service = new UserService();
                              ^
  symbol:   class UserService
  location: class UserServiceTest
/src/test/java/com/example/UserServiceTest.java:22: error: method create in class UserService cannot be applied to given types;
    service.create(user);
           ^
  required: String,String
  found: User
1 error

FAILURE: Build failed with an exception.

* What went wrong:
Execution failed for task ':compileTestJava'.
> Compilation failed; see the compiler error output for details.

* Try:
> Run with --stacktrace option to get the stack trace.
> Run with --info or --debug option to get more log output.
> Run with --scan to get full insights.

BUILD FAILED in 5s
3 actionable tasks: 3 executed";

    const TEST_FAILURES: &str = "\
> Task :compileJava UP-TO-DATE
> Task :processResources NO-SOURCE
> Task :classes UP-TO-DATE
> Task :compileTestJava UP-TO-DATE
> Task :processTestResources NO-SOURCE
> Task :testClasses UP-TO-DATE

> Task :test FAILED

com.example.UserServiceTest > testCreate FAILED
    java.lang.AssertionError: expected:<200> but was:<404>
        at org.junit.Assert.fail(Assert.java:89)
        at org.junit.Assert.failNotEquals(Assert.java:835)
        at org.junit.Assert.assertEquals(Assert.java:120)
        at com.example.UserServiceTest.testCreate(UserServiceTest.java:25)

com.example.OrderTest > testCancel FAILED
    java.lang.NullPointerException
        at com.example.OrderTest.testCancel(OrderTest.java:42)

com.example.UserServiceTest > testGet PASSED
com.example.UserServiceTest > testDelete PASSED
com.example.OrderTest > testCreate PASSED
com.example.OrderTest > testUpdate PASSED

4 tests completed, 2 failed

FAILURE: Build failed with an exception.

* What went wrong:
Execution failed for task ':test'.
> There were failing tests. See the report at: file:///build/reports/tests/test/index.html

* Try:
> Run with --stacktrace option to get the stack trace.
> Run with --info or --debug option to get more log output.
> Run with --scan to get full insights.

BUILD FAILED in 8s
5 actionable tasks: 1 executed, 4 up-to-date";

    const INFO_VERBOSE: &str = "\
Initialized native services in: /home/user/.gradle/native
Initialized jansi services in: /home/user/.gradle/native
The client will now receive all logging from the daemon (pid: 12345). The daemon log file: /home/user/.gradle/daemon/7.6/daemon-12345.out.log
Starting 8th build in daemon [uptime: 2 mins 30.456 secs, performance: 98%, non-heap usage: 25% of 268 MiB]
Using 8 worker leases.
Now considering [/home/user/project] as hierarchies to watch
Watching the file system is configured to be enabled if available
File system watching is active
Starting Build
Settings evaluated using settings file '/home/user/project/settings.gradle'.
Projects loaded. Root project using build file '/home/user/project/build.gradle'.
Included projects: [root project 'myproject', project ':app', project ':lib']

> Configure project :
Evaluating root project 'myproject' using build file '/home/user/project/build.gradle'.
Build cache is an incubating feature.
All projects evaluated.
Task name matched 'build'
Selected primary task 'build' from project :

> Task :compileJava UP-TO-DATE
Resolving global dependency management for project ':'
Caching disabled for task ':compileJava' because:
  Build cache is disabled
Skipping task ':compileJava' as it is up-to-date.

> Task :processResources NO-SOURCE
Skipping task ':processResources' as it has no source files and no previous output files.

> Task :classes UP-TO-DATE
Skipping task ':classes' as it has no actions.

> Task :jar UP-TO-DATE
Skipping task ':jar' as it is up-to-date.

> Task :assemble UP-TO-DATE
Skipping task ':assemble' as it has no actions.

> Task :compileTestJava UP-TO-DATE
Skipping task ':compileTestJava' as it is up-to-date.

> Task :processTestResources NO-SOURCE
Skipping task ':processTestResources' as it has no source files and no previous output files.

> Task :testClasses UP-TO-DATE
Skipping task ':testClasses' as it has no actions.

> Task :test UP-TO-DATE
Skipping task ':test' as it is up-to-date.

> Task :check UP-TO-DATE
Skipping task ':check' as it has no actions.

> Task :build UP-TO-DATE
Skipping task ':build' as it has no actions.

BUILD SUCCESSFUL in 1s
7 actionable tasks: 7 up-to-date";

    const DEPENDENCY_TREE: &str = "\
> Task :dependencies

------------------------------------------------------------
Root project 'myproject'
------------------------------------------------------------

compileClasspath - Compile classpath for source set 'main'.
+--- org.springframework.boot:spring-boot-starter-web:3.2.0
|    +--- org.springframework.boot:spring-boot-starter:3.2.0
|    |    +--- org.springframework.boot:spring-boot:3.2.0
|    |    |    +--- org.springframework:spring-core:6.1.1
|    |    |    |    \\--- org.springframework:spring-jcl:6.1.1
|    |    |    \\--- org.springframework:spring-context:6.1.1
|    |    |         +--- org.springframework:spring-aop:6.1.1
|    |    |         |    +--- org.springframework:spring-beans:6.1.1
|    |    |         |    |    \\--- org.springframework:spring-core:6.1.1 (*)
|    |    |         |    \\--- org.springframework:spring-core:6.1.1 (*)
|    |    |         +--- org.springframework:spring-beans:6.1.1 (*)
|    |    |         +--- org.springframework:spring-core:6.1.1 (*)
|    |    |         \\--- org.springframework:spring-expression:6.1.1
|    |    |              \\--- org.springframework:spring-core:6.1.1 (*)
|    |    +--- org.springframework.boot:spring-boot-autoconfigure:3.2.0
|    |    |    \\--- org.springframework.boot:spring-boot:3.2.0 (*)
|    |    +--- org.springframework.boot:spring-boot-starter-logging:3.2.0
|    |    |    +--- ch.qos.logback:logback-classic:1.4.11
|    |    |    |    +--- ch.qos.logback:logback-core:1.4.11
|    |    |    |    \\--- org.slf4j:slf4j-api:2.0.9
|    |    |    +--- org.apache.logging.log4j:log4j-to-slf4j:2.21.0
|    |    |    |    +--- org.apache.logging.log4j:log4j-api:2.21.0
|    |    |    |    \\--- org.slf4j:slf4j-api:2.0.9
|    |    |    \\--- org.slf4j:jul-to-slf4j:2.0.9
|    |    |         \\--- org.slf4j:slf4j-api:2.0.9
|    |    +--- jakarta.annotation:jakarta.annotation-api:2.1.1
|    |    \\--- org.yaml:snakeyaml:2.2
|    +--- org.springframework.boot:spring-boot-starter-json:3.2.0
|    |    +--- org.springframework.boot:spring-boot-starter:3.2.0 (*)
|    |    +--- org.springframework:spring-web:6.1.1
|    |    |    +--- org.springframework:spring-beans:6.1.1 (*)
|    |    |    +--- org.springframework:spring-core:6.1.1 (*)
|    |    |    \\--- io.micrometer:micrometer-observation:1.12.0
|    |    +--- com.fasterxml.jackson.core:jackson-databind:2.15.3
|    |    +--- com.fasterxml.jackson.datatype:jackson-datatype-jdk8:2.15.3
|    |    \\--- com.fasterxml.jackson.datatype:jackson-datatype-jsr310:2.15.3
|    +--- org.springframework:spring-web:6.1.1 (*)
|    \\--- org.springframework.boot:spring-boot-starter-tomcat:3.2.0
|         +--- jakarta.annotation:jakarta.annotation-api:2.1.1
|         +--- org.apache.tomcat.embed:tomcat-embed-core:10.1.16
|         \\--- org.apache.tomcat.embed:tomcat-embed-websocket:10.1.16
+--- org.projectlombok:lombok:1.18.30
\\--- com.google.guava:guava:32.1.3-jre

BUILD SUCCESSFUL in 1s
1 actionable task: 1 executed";

    const DEPRECATION_WARNINGS: &str = "\
> Task :compileJava
Note: /src/main/java/com/example/OldService.java uses or overrides a deprecated API.
Note: Recompile with -Xlint:deprecation for details.
Note: /src/main/java/com/example/LegacyDao.java uses or overrides a deprecated API.
Note: Recompile with -Xlint:deprecation for details.
Note: /src/main/java/com/example/OldController.java uses or overrides a deprecated API.
Note: Recompile with -Xlint:deprecation for details.

> Task :processResources UP-TO-DATE
> Task :classes
> Task :jar
> Task :assemble

BUILD SUCCESSFUL in 4s
4 actionable tasks: 2 executed, 2 up-to-date";

    const DOWNLOAD_NOISE: &str = "\
Downloading https://services.gradle.org/distributions/gradle-8.5-bin.zip
...........10%............20%............30%.............40%............50%............60%............70%.............80%............90%............100%

Welcome to Gradle 8.5!

Here are the highlights of this release:
 - Support for running on Java 21
 - Faster first use with Kotlin DSL

For more details see https://docs.gradle.org/8.5/release-notes.html

Starting a Gradle Daemon (subsequent builds will be faster)

> Task :compileJava UP-TO-DATE
> Task :classes UP-TO-DATE
> Task :jar UP-TO-DATE

BUILD SUCCESSFUL in 45s
3 actionable tasks: 3 up-to-date";

    const ETENDO_SMARTBUILD: &str = "\
> Configure project :
Build cache is an incubating feature.

> Task :compileJava UP-TO-DATE
> Task :processResources UP-TO-DATE
> Task :classes UP-TO-DATE
> Task :core:compileJava UP-TO-DATE
> Task :core:processResources UP-TO-DATE
> Task :core:classes UP-TO-DATE
> Task :modules:com.etendo.mymodule:compileJava
> Task :modules:com.etendo.mymodule:processResources NO-SOURCE
> Task :modules:com.etendo.mymodule:classes
> Task :smartbuild
[ant:smartbuild] Smartbuild started
[ant:smartbuild] Compiling module com.etendo.mymodule
[ant:smartbuild] Generating entities for module com.etendo.mymodule
[ant:smartbuild] Updating database for module com.etendo.mymodule
[ant:smartbuild] Database updated successfully
[ant:smartbuild] Deploying to tomcat
[ant:smartbuild] Tomcat deployment complete
[ant:smartbuild] Smartbuild completed successfully

BUILD SUCCESSFUL in 45s
12 actionable tasks: 4 executed, 8 up-to-date";

    #[test]
    fn test_filter_build_success_removes_task_lines() {
        let filtered = filter_gradle_output(BUILD_SUCCESS);
        // Should NOT contain task progress lines
        assert!(
            !filtered.contains("> Task :compileJava UP-TO-DATE"),
            "Should remove UP-TO-DATE task lines"
        );
        assert!(
            !filtered.contains("> Task :processResources NO-SOURCE"),
            "Should remove NO-SOURCE task lines"
        );
        // Should keep BUILD SUCCESSFUL
        assert!(
            filtered.contains("BUILD SUCCESSFUL"),
            "Should keep build summary"
        );
    }

    #[test]
    fn test_filter_build_success_removes_configure() {
        let filtered = filter_gradle_output(BUILD_SUCCESS);
        assert!(
            !filtered.contains("> Configure project"),
            "Should remove configure lines"
        );
        assert!(
            !filtered.contains("Build cache is an incubating feature"),
            "Should remove incubating feature noise"
        );
    }

    #[test]
    fn test_filter_build_failed_keeps_errors() {
        let filtered = filter_gradle_output(BUILD_FAILED);
        assert!(
            filtered.contains("cannot find symbol"),
            "Should keep compiler errors"
        );
        assert!(
            filtered.contains("BUILD FAILED"),
            "Should keep build failure summary"
        );
        assert!(
            filtered.contains("FAILED"),
            "Should keep FAILED task indicator"
        );
    }

    #[test]
    fn test_filter_build_failed_removes_try_suggestions() {
        let filtered = filter_gradle_output(BUILD_FAILED);
        assert!(
            !filtered.contains("Run with --stacktrace"),
            "Should remove Try suggestions"
        );
        assert!(
            !filtered.contains("Run with --info"),
            "Should remove Try suggestions"
        );
        assert!(
            !filtered.contains("Run with --scan"),
            "Should remove Try suggestions"
        );
    }

    #[test]
    fn test_filter_test_failures_keeps_failed_tests() {
        let filtered = filter_gradle_output(TEST_FAILURES);
        assert!(
            filtered.contains("testCreate FAILED"),
            "Should keep failed test name"
        );
        assert!(
            filtered.contains("testCancel FAILED"),
            "Should keep failed test name"
        );
        assert!(
            filtered.contains("AssertionError"),
            "Should keep error message"
        );
        assert!(
            filtered.contains("NullPointerException"),
            "Should keep error type"
        );
    }

    #[test]
    fn test_filter_test_failures_removes_passed() {
        let filtered = filter_gradle_output(TEST_FAILURES);
        assert!(
            !filtered.contains("testGet PASSED"),
            "Should remove passed tests"
        );
        assert!(
            !filtered.contains("testDelete PASSED"),
            "Should remove passed tests"
        );
    }

    #[test]
    fn test_filter_test_failures_keeps_summary() {
        let filtered = filter_gradle_output(TEST_FAILURES);
        assert!(
            filtered.contains("4 tests completed, 2 failed"),
            "Should keep test summary"
        );
    }

    #[test]
    fn test_filter_info_verbose_removes_daemon_noise() {
        let filtered = filter_gradle_output(INFO_VERBOSE);
        assert!(
            !filtered.contains("Initialized native services"),
            "Should remove daemon init"
        );
        assert!(
            !filtered.contains("daemon log file"),
            "Should remove daemon log path"
        );
        assert!(
            !filtered.contains("Starting 8th build in daemon"),
            "Should remove daemon stats"
        );
        assert!(
            !filtered.contains("worker leases"),
            "Should remove worker info"
        );
        assert!(
            !filtered.contains("File system watching"),
            "Should remove fs watch info"
        );
    }

    #[test]
    fn test_filter_info_verbose_removes_evaluation_noise() {
        let filtered = filter_gradle_output(INFO_VERBOSE);
        assert!(
            !filtered.contains("Evaluating root project"),
            "Should remove project evaluation"
        );
        assert!(
            !filtered.contains("All projects evaluated"),
            "Should remove evaluation summary"
        );
        assert!(
            !filtered.contains("Task name matched"),
            "Should remove task matching"
        );
        assert!(
            !filtered.contains("Selected primary task"),
            "Should remove task selection"
        );
        assert!(
            !filtered.contains("Caching disabled"),
            "Should remove cache info"
        );
        assert!(
            !filtered.contains("Skipping task"),
            "Should remove skip reasons"
        );
    }

    #[test]
    fn test_filter_info_verbose_keeps_summary() {
        let filtered = filter_gradle_output(INFO_VERBOSE);
        assert!(
            filtered.contains("BUILD SUCCESSFUL"),
            "Should keep build summary even in --info mode"
        );
    }

    #[test]
    fn test_filter_dependency_tree_compacts() {
        let filtered = filter_gradle_output(DEPENDENCY_TREE);
        let input_tokens = count_tokens(DEPENDENCY_TREE);
        let output_tokens = count_tokens(&filtered);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Dependency tree should have >=60% savings, got {:.1}%",
            savings
        );
        // Should keep top-level deps
        assert!(
            filtered.contains("spring-boot-starter-web"),
            "Should keep top-level dependency"
        );
        assert!(
            filtered.contains("lombok"),
            "Should keep top-level dependency"
        );
        assert!(
            filtered.contains("guava"),
            "Should keep top-level dependency"
        );
    }

    #[test]
    fn test_filter_deprecation_warnings_groups() {
        let filtered = filter_gradle_output(DEPRECATION_WARNINGS);
        // Should not have 3 separate identical "Note: Recompile with" lines
        let recompile_count = filtered.matches("Recompile with").count();
        assert!(
            recompile_count <= 1,
            "Should deduplicate deprecation notes, got {} occurrences",
            recompile_count
        );
        assert!(
            filtered.contains("BUILD SUCCESSFUL"),
            "Should keep build summary"
        );
    }

    #[test]
    fn test_filter_download_noise() {
        let filtered = filter_gradle_output(DOWNLOAD_NOISE);
        assert!(
            !filtered.contains("...........10%"),
            "Should remove download progress"
        );
        assert!(
            !filtered.contains("Welcome to Gradle"),
            "Should remove welcome banner"
        );
        assert!(
            !filtered.contains("highlights of this release"),
            "Should remove release notes"
        );
        assert!(
            !filtered.contains("Starting a Gradle Daemon"),
            "Should remove daemon start message"
        );
        assert!(
            filtered.contains("BUILD SUCCESSFUL"),
            "Should keep build summary"
        );
    }

    #[test]
    fn test_filter_etendo_smartbuild_keeps_ant_output() {
        let filtered = filter_gradle_output(ETENDO_SMARTBUILD);
        assert!(
            filtered.contains("Smartbuild started"),
            "Should keep [ant:] output lines"
        );
        assert!(
            filtered.contains("Smartbuild completed successfully"),
            "Should keep [ant:] completion"
        );
        assert!(
            filtered.contains("BUILD SUCCESSFUL"),
            "Should keep build summary"
        );
        // Should still remove UP-TO-DATE noise
        assert!(
            !filtered.contains("> Task :compileJava UP-TO-DATE"),
            "Should remove UP-TO-DATE tasks"
        );
    }

    #[test]
    fn test_filter_build_success_token_savings() {
        let filtered = filter_gradle_output(BUILD_SUCCESS);
        let input_tokens = count_tokens(BUILD_SUCCESS);
        let output_tokens = count_tokens(&filtered);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Build success should have >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_info_verbose_token_savings() {
        let filtered = filter_gradle_output(INFO_VERBOSE);
        let input_tokens = count_tokens(INFO_VERBOSE);
        let output_tokens = count_tokens(&filtered);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 80.0,
            "--info verbose should have >=80% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_empty_input() {
        let filtered = filter_gradle_output("");
        assert_eq!(filtered, "");
    }

    #[test]
    fn test_filter_only_build_summary() {
        let input = "BUILD SUCCESSFUL in 0s";
        let filtered = filter_gradle_output(input);
        assert!(filtered.contains("BUILD SUCCESSFUL"));
    }

    #[test]
    fn test_detect_gradle_binary_fallback() {
        // When not in a directory with gradlew, should fall back to gradle
        // (this test runs from the rtk project root which has no gradlew)
        let bin = detect_gradle_binary();
        assert_eq!(bin, "gradle");
    }

    // --- Snapshot tests ---

    #[test]
    fn test_snapshot_build_success() {
        assert_snapshot!(filter_gradle_output(BUILD_SUCCESS));
    }

    #[test]
    fn test_snapshot_build_failed() {
        assert_snapshot!(filter_gradle_output(BUILD_FAILED));
    }

    #[test]
    fn test_snapshot_test_failures() {
        assert_snapshot!(filter_gradle_output(TEST_FAILURES));
    }

    #[test]
    fn test_snapshot_info_verbose() {
        assert_snapshot!(filter_gradle_output(INFO_VERBOSE));
    }

    #[test]
    fn test_snapshot_dependency_tree() {
        assert_snapshot!(filter_gradle_output(DEPENDENCY_TREE));
    }

    #[test]
    fn test_snapshot_etendo_smartbuild() {
        assert_snapshot!(filter_gradle_output(ETENDO_SMARTBUILD));
    }
}
