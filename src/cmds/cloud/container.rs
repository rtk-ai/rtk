//! Filters Docker and kubectl output into compact summaries.

use crate::core::runner::{self, RunOptions};
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use serde_json::Value;
use std::ffi::OsString;
use std::process::Command;

#[derive(Debug, Clone, Copy)]
pub enum ContainerCmd {
    DockerPs,
    DockerImages,
    DockerLogs,
    KubectlPods,
    KubectlServices,
    KubectlLogs,
}

pub fn run(cmd: ContainerCmd, args: &[String], verbose: u8) -> Result<i32> {
    match cmd {
        ContainerCmd::DockerPs => docker_ps(verbose),
        ContainerCmd::DockerImages => docker_images(verbose),
        ContainerCmd::DockerLogs => docker_logs(args, verbose),
        ContainerCmd::KubectlPods => kubectl_pods(args, verbose),
        ContainerCmd::KubectlServices => kubectl_services(args, verbose),
        ContainerCmd::KubectlLogs => kubectl_logs(args, verbose),
    }
}

fn run_kubectl_json<F>(cmd: Command, label: &str, filter_fn: F) -> Result<i32>
where
    F: Fn(&Value) -> String,
{
    runner::run_filtered(
        cmd,
        "kubectl",
        label,
        |stdout| match serde_json::from_str::<Value>(stdout) {
            Ok(json) => filter_fn(&json),
            Err(e) => {
                eprintln!("[rtk] kubectl: JSON parse failed: {}", e);
                stdout.to_string()
            }
        },
        RunOptions::stdout_only()
            .early_exit_on_failure()
            .no_trailing_newline(),
    )
}

fn docker_ps(_verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let raw = exec_capture(resolved_command("docker").args(["ps"]))
        .map(|r| r.stdout)
        .unwrap_or_default();

    let result = exec_capture(resolved_command("docker").args([
        "ps",
        "--format",
        "{{.ID}}\t{{.Names}}\t{{.Status}}\t{{.Image}}\t{{.Ports}}",
    ]))
    .context("Failed to run docker ps")?;

    if !result.success() {
        eprint!("{}", result.stderr);
        timer.track("docker ps", "rtk docker ps", &raw, &raw);
        return Ok(result.exit_code);
    }

    let stdout = result.stdout;
    let mut rtk = String::new();

    if stdout.trim().is_empty() {
        rtk.push_str("[docker] 0 containers");
        println!("{}", rtk);
        timer.track("docker ps", "rtk docker ps", &raw, &rtk);
        return Ok(0);
    }

    let count = stdout.lines().count();
    rtk.push_str(&format!("[docker] {} containers:\n", count));

    for line in stdout.lines().take(15) {
        let parts: Vec<&str> = line.split('\t').collect();
        if let Some(formatted) = format_docker_ps_line(&parts) {
            rtk.push_str(&formatted);
        }
    }
    if count > 15 {
        rtk.push_str(&format!("  ... +{} more", count - 15));
    }

    print!("{}", rtk);
    timer.track("docker ps", "rtk docker ps", &raw, &rtk);
    Ok(0)
}

fn docker_images(_verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let raw = exec_capture(resolved_command("docker").args(["images"]))
        .map(|r| r.stdout)
        .unwrap_or_default();

    let result = exec_capture(resolved_command("docker").args([
        "images",
        "--format",
        "{{.Repository}}:{{.Tag}}\t{{.Size}}",
    ]))
    .context("Failed to run docker images")?;

    if !result.success() {
        eprint!("{}", result.stderr);
        timer.track("docker images", "rtk docker images", &raw, &raw);
        return Ok(result.exit_code);
    }

    let stdout = result.stdout;
    let lines: Vec<&str> = stdout.lines().collect();
    let mut rtk = String::new();

    if lines.is_empty() {
        rtk.push_str("[docker] 0 images");
        println!("{}", rtk);
        timer.track("docker images", "rtk docker images", &raw, &rtk);
        return Ok(0);
    }

    let mut total_size_mb: f64 = 0.0;
    for line in &lines {
        let parts: Vec<&str> = line.split('\t').collect();
        if let Some(size_str) = parts.get(1) {
            if size_str.contains("GB") {
                if let Ok(n) = size_str.replace("GB", "").trim().parse::<f64>() {
                    total_size_mb += n * 1024.0;
                }
            } else if size_str.contains("MB") {
                if let Ok(n) = size_str.replace("MB", "").trim().parse::<f64>() {
                    total_size_mb += n;
                }
            }
        }
    }

    let total_display = if total_size_mb > 1024.0 {
        format!("{:.1}GB", total_size_mb / 1024.0)
    } else {
        format!("{:.0}MB", total_size_mb)
    };
    rtk.push_str(&format!(
        "[docker] {} images ({})\n",
        lines.len(),
        total_display
    ));

    for line in lines.iter().take(15) {
        let parts: Vec<&str> = line.split('\t').collect();
        if !parts.is_empty() {
            let image = parts[0];
            let size = parts.get(1).unwrap_or(&"");
            let short = if image.len() > 40 {
                format!("...{}", &image[image.len() - 37..])
            } else {
                image.to_string()
            };
            rtk.push_str(&format!("  {} [{}]\n", short, size));
        }
    }
    if lines.len() > 15 {
        rtk.push_str(&format!("  ... +{} more", lines.len() - 15));
    }

    print!("{}", rtk);
    timer.track("docker images", "rtk docker images", &raw, &rtk);
    Ok(0)
}

fn docker_logs(args: &[String], _verbose: u8) -> Result<i32> {
    let container = args.first().map(|s| s.as_str()).unwrap_or("");
    if container.is_empty() {
        println!("Usage: rtk docker logs <container>");
        return Ok(0);
    }

    let mut cmd = resolved_command("docker");
    cmd.args(["logs", "--tail", "100", container]);

    let label = format!("logs {}", container);
    runner::run_filtered(
        cmd,
        "docker",
        &label,
        |raw| {
            format!(
                "[docker] Logs for {}:\n{}",
                container,
                crate::log_cmd::run_stdin_str(raw)
            )
        },
        RunOptions::default().early_exit_on_failure(),
    )
}

fn kubectl_pods(args: &[String], _verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("kubectl");
    cmd.args(["get", "pods", "-o", "json"]);
    for arg in args {
        cmd.arg(arg);
    }
    run_kubectl_json(cmd, "get pods", format_kubectl_pods)
}

fn format_kubectl_pods(json: &Value) -> String {
    let Some(pods) = json["items"].as_array().filter(|a| !a.is_empty()) else {
        return "No pods found\n".to_string();
    };
    let (mut running, mut pending, mut failed, mut restarts_total) = (0, 0, 0, 0i64);
    let mut issues: Vec<String> = Vec::new();

    for pod in pods {
        let ns = pod["metadata"]["namespace"].as_str().unwrap_or("-");
        let name = pod["metadata"]["name"].as_str().unwrap_or("-");
        let phase = pod["status"]["phase"].as_str().unwrap_or("Unknown");

        if let Some(containers) = pod["status"]["containerStatuses"].as_array() {
            for c in containers {
                restarts_total += c["restartCount"].as_i64().unwrap_or(0);
            }
        }

        match phase {
            "Running" => running += 1,
            "Pending" => {
                pending += 1;
                issues.push(format!("{}/{} Pending", ns, name));
            }
            "Failed" | "Error" => {
                failed += 1;
                issues.push(format!("{}/{} {}", ns, name, phase));
            }
            _ => {
                if let Some(containers) = pod["status"]["containerStatuses"].as_array() {
                    for c in containers {
                        if let Some(w) = c["state"]["waiting"]["reason"].as_str() {
                            if w.contains("CrashLoop") || w.contains("Error") {
                                failed += 1;
                                issues.push(format!("{}/{} {}", ns, name, w));
                            }
                        }
                    }
                }
            }
        }
    }

    let mut parts = Vec::new();
    if running > 0 {
        parts.push(format!("{}", running));
    }
    if pending > 0 {
        parts.push(format!("{} pending", pending));
    }
    if failed > 0 {
        parts.push(format!("{} [x]", failed));
    }
    if restarts_total > 0 {
        parts.push(format!("{} restarts", restarts_total));
    }

    let mut out = format!("{} pods: {}\n", pods.len(), parts.join(", "));
    if !issues.is_empty() {
        out.push_str("[warn] Issues:\n");
        for issue in issues.iter().take(10) {
            out.push_str(&format!("  {}\n", issue));
        }
        if issues.len() > 10 {
            out.push_str(&format!("  ... +{} more", issues.len() - 10));
        }
    }
    out
}

fn kubectl_services(args: &[String], _verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("kubectl");
    cmd.args(["get", "services", "-o", "json"]);
    for arg in args {
        cmd.arg(arg);
    }
    run_kubectl_json(cmd, "get services", format_kubectl_services)
}

fn format_kubectl_services(json: &Value) -> String {
    let Some(services) = json["items"].as_array().filter(|a| !a.is_empty()) else {
        return "No services found\n".to_string();
    };
    let mut out = format!("{} services:\n", services.len());

    for svc in services.iter().take(15) {
        let ns = svc["metadata"]["namespace"].as_str().unwrap_or("-");
        let name = svc["metadata"]["name"].as_str().unwrap_or("-");
        let svc_type = svc["spec"]["type"].as_str().unwrap_or("-");
        let ports: Vec<String> = svc["spec"]["ports"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|p| {
                        let port = p["port"].as_i64().unwrap_or(0);
                        let target = p["targetPort"]
                            .as_i64()
                            .or_else(|| p["targetPort"].as_str().and_then(|s| s.parse().ok()))
                            .unwrap_or(port);
                        if port == target {
                            format!("{}", port)
                        } else {
                            format!("{}→{}", port, target)
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push_str(&format!(
            "  {}/{} {} [{}]\n",
            ns,
            name,
            svc_type,
            ports.join(",")
        ));
    }
    if services.len() > 15 {
        out.push_str(&format!("  ... +{} more", services.len() - 15));
    }
    out
}

fn kubectl_logs(args: &[String], _verbose: u8) -> Result<i32> {
    let pod = args.first().map(|s| s.as_str()).unwrap_or("");
    if pod.is_empty() {
        println!("Usage: rtk kubectl logs <pod>");
        return Ok(0);
    }

    let mut cmd = resolved_command("kubectl");
    cmd.args(["logs", "--tail", "100", pod]);
    for arg in args.iter().skip(1) {
        cmd.arg(arg);
    }

    let label = format!("logs {}", pod);
    runner::run_filtered(
        cmd,
        "kubectl",
        &label,
        |stdout| {
            format!(
                "Logs for {}:\n{}",
                pod,
                crate::log_cmd::run_stdin_str(stdout)
            )
        },
        RunOptions::stdout_only().early_exit_on_failure(),
    )
}

/// Format `docker compose ps --format` output into compact form.
/// Expects tab-separated lines: Name\tImage\tStatus\tPorts
/// (no header row — `--format` output is headerless)
pub fn format_compose_ps(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();

    if lines.is_empty() {
        return "[compose] 0 services".to_string();
    }

    let mut result = format!("[compose] {} services:\n", lines.len());

    for line in lines.iter().take(20) {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 4 {
            let name = parts[0];
            let image = parts[1];
            let status = parts[2];
            let ports = parts[3];

            let short_image = trim_image_registry(image);

            let port_str = if ports.trim().is_empty() {
                String::new()
            } else {
                let compact = compact_ports(ports.trim());
                if compact == "-" {
                    String::new()
                } else {
                    format!(" [{}]", compact)
                }
            };

            result.push_str(&format!(
                "  {} ({}) {}{}\n",
                name, short_image, status, port_str
            ));
        }
    }
    if lines.len() > 20 {
        result.push_str(&format!("  ... +{} more\n", lines.len() - 20));
    }

    result.trim_end().to_string()
}

/// Format `docker compose logs` output into compact form
pub fn format_compose_logs(raw: &str) -> String {
    if raw.trim().is_empty() {
        return "[compose] No logs".to_string();
    }

    // docker compose logs prefixes each line with "service-N  | "
    // Use the existing log deduplication engine
    let analyzed = crate::log_cmd::run_stdin_str(raw);
    format!("[compose] Logs:\n{}", analyzed)
}

/// Format `docker compose build` output into compact summary
pub fn format_compose_build(raw: &str) -> String {
    if raw.trim().is_empty() {
        return "[compose] Build: no output".to_string();
    }

    let mut result = String::new();

    // Extract the summary line: "[+] Building 12.3s (8/8) FINISHED"
    for line in raw.lines() {
        if line.contains("Building") && line.contains("FINISHED") {
            result.push_str(&format!("[compose] {}\n", line.trim()));
            break;
        }
    }

    if result.is_empty() {
        // No FINISHED line found — might still be building or errored
        if let Some(line) = raw.lines().find(|l| l.contains("Building")) {
            result.push_str(&format!("[compose] {}\n", line.trim()));
        } else {
            result.push_str("[compose] Build:\n");
        }
    }

    // Collect unique service names from build steps like "[web 1/4]"
    let mut services: Vec<String> = Vec::new();
    // find('[') returns byte offset — use byte slicing throughout
    // '[' and ']' are single-byte ASCII, so byte arithmetic is safe
    for line in raw.lines() {
        if let Some(start) = line.find('[') {
            if let Some(end) = line[start + 1..].find(']') {
                let bracket = &line[start + 1..start + 1 + end];
                let svc = bracket.split_whitespace().next().unwrap_or("");
                if !svc.is_empty() && svc != "+" && !services.contains(&svc.to_string()) {
                    services.push(svc.to_string());
                }
            }
        }
    }

    if !services.is_empty() {
        result.push_str(&format!("  Services: {}\n", services.join(", ")));
    }

    // Count build steps (lines starting with " => ")
    let step_count = raw
        .lines()
        .filter(|l| l.trim_start().starts_with("=> "))
        .count();
    if step_count > 0 {
        result.push_str(&format!("  Steps: {}", step_count));
    }

    result.trim_end().to_string()
}

/// Prefix added to status when a container's health check is failing.
const UNHEALTHY_PREFIX: &str = "[!]";

/// Format a single tab-separated `docker ps --format` line. Returns `None` for malformed input.
pub(crate) fn format_docker_ps_line(parts: &[&str]) -> Option<String> {
    if parts.len() < 4 {
        return None;
    }
    let id = &parts[0][..12.min(parts[0].len())];
    let name = parts[1];
    let raw_status = parts[2].trim();
    let status = if raw_status.contains("(unhealthy)") {
        format!("{} {}", UNHEALTHY_PREFIX, raw_status)
    } else {
        raw_status.to_string()
    };
    let short_img = trim_image_registry(parts[3]);
    let ports = compact_ports(parts.get(4).unwrap_or(&""));
    let line = if ports == "-" {
        format!("  {} {} {} ({})\n", id, name, status, short_img)
    } else {
        format!("  {} {} {} ({}) [{}]\n", id, name, status, short_img, ports)
    };
    Some(line)
}

/// Strip the registry/namespace prefix from an image name, e.g. `registry.io/org/nginx:1` → `nginx:1`.
fn trim_image_registry(image: &str) -> &str {
    image.split('/').next_back().unwrap_or(image)
}

fn compact_ports(ports: &str) -> String {
    if ports.is_empty() {
        return "-".to_string();
    }

    // Extract just the port numbers
    let port_nums: Vec<&str> = ports
        .split(',')
        .filter_map(|p| p.split("->").next().and_then(|s| s.split(':').next_back()))
        .collect();

    if port_nums.len() <= 3 {
        port_nums.join(", ")
    } else {
        format!(
            "{}, ... +{}",
            port_nums[..2].join(", "),
            port_nums.len() - 2
        )
    }
}

pub fn run_docker_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    crate::core::runner::run_passthrough("docker", args, verbose)
}

/// Run `docker compose ps` with compact output
pub fn run_compose_ps(verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    // Raw output for token tracking
    let raw_result = exec_capture(resolved_command("docker").args(["compose", "ps"]))
        .context("Failed to run docker compose ps")?;

    if !raw_result.success() {
        eprintln!("{}", raw_result.stderr);
        return Ok(raw_result.exit_code);
    }
    let raw = raw_result.stdout;

    // Structured output for parsing (same pattern as docker_ps)
    let result = exec_capture(resolved_command("docker").args([
        "compose",
        "ps",
        "--format",
        "{{.Name}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}",
    ]))
    .context("Failed to run docker compose ps --format")?;

    if !result.success() {
        eprintln!("{}", result.stderr);
        return Ok(result.exit_code);
    }
    let structured = result.stdout;

    if verbose > 0 {
        eprintln!("raw docker compose ps:\n{}", raw);
    }

    let rtk = format_compose_ps(&structured);
    println!("{}", rtk);
    timer.track("docker compose ps", "rtk docker compose ps", &raw, &rtk);
    Ok(0)
}

pub fn run_compose_logs(service: Option<&str>, verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("docker");
    cmd.args(["compose", "logs", "--tail", "100"]);
    if let Some(svc) = service {
        cmd.arg(svc);
    }

    let svc_label = service.unwrap_or("all");
    runner::run_filtered(
        cmd,
        "docker",
        &format!("compose logs {}", svc_label),
        |raw| {
            if verbose > 0 {
                eprintln!("raw docker compose logs:\n{}", raw);
            }
            format_compose_logs(raw)
        },
        RunOptions::default().early_exit_on_failure(),
    )
}

pub fn run_compose_build(service: Option<&str>, verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("docker");
    cmd.args(["compose", "build"]);
    if let Some(svc) = service {
        cmd.arg(svc);
    }

    let svc_label = service.unwrap_or("all");
    runner::run_filtered(
        cmd,
        "docker",
        &format!("compose build {}", svc_label),
        |raw| {
            if verbose > 0 {
                eprintln!("raw docker compose build:\n{}", raw);
            }
            format_compose_build(raw)
        },
        RunOptions::default().early_exit_on_failure(),
    )
}

pub fn run_compose_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    let mut combined = vec![OsString::from("compose")];
    combined.extend_from_slice(args);
    crate::core::runner::run_passthrough("docker", &combined, verbose)
}

pub fn run_kubectl_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    crate::core::runner::run_passthrough("kubectl", args, verbose)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── format_compose_ps ──────────────────────────────────

    #[test]
    fn test_format_compose_ps_basic() {
        // Tab-separated --format output: Name\tImage\tStatus\tPorts
        let raw = "web-1\tnginx:latest\tUp 2 hours\t0.0.0.0:80->80/tcp\n\
                   api-1\tnode:20\tUp 2 hours\t0.0.0.0:3000->3000/tcp\n\
                   db-1\tpostgres:16\tUp 2 hours\t0.0.0.0:5432->5432/tcp";
        let out = format_compose_ps(raw);
        assert!(out.contains("3"), "should show container count");
        assert!(out.contains("web"), "should show service name");
        assert!(out.contains("api"), "should show service name");
        assert!(out.contains("db"), "should show service name");
        assert!(out.contains("Up 2 hours"), "should show status");
        assert!(out.len() < raw.len(), "output should be shorter than raw");
    }

    #[test]
    fn test_format_compose_ps_empty() {
        let out = format_compose_ps("");
        assert!(out.contains("0"), "should show zero containers");
    }

    #[test]
    fn test_format_compose_ps_whitespace_only() {
        let out = format_compose_ps("   \n  \n");
        assert!(out.contains("0"), "should show zero containers");
    }

    #[test]
    fn test_format_compose_ps_exited_service() {
        // Tab-separated --format output
        let raw = "worker-1\tpython:3.12\tExited (1) 2 minutes ago\t";
        let out = format_compose_ps(raw);
        assert!(out.contains("worker"), "should show service name");
        assert!(out.contains("Exited"), "should show exited status");
    }

    #[test]
    fn test_format_compose_ps_no_ports() {
        let raw = "redis-1\tredis:7\tUp 5 hours\t";
        let out = format_compose_ps(raw);
        assert!(out.contains("redis"), "should show service name");
        // Should not show port info when no ports (but [compose] prefix is OK)
        let lines: Vec<&str> = out.lines().collect();
        let redis_line = lines.iter().find(|l| l.contains("redis")).unwrap();
        assert!(
            !redis_line.contains("] ["),
            "should not show port brackets when empty"
        );
    }

    #[test]
    fn test_format_compose_ps_long_image_path() {
        let raw = "app-1\tghcr.io/myorg/myapp:latest\tUp 1 hour\t0.0.0.0:8080->8080/tcp";
        let out = format_compose_ps(raw);
        assert!(
            out.contains("myapp:latest"),
            "should shorten image to last segment"
        );
        assert!(
            !out.contains("ghcr.io"),
            "should not show full registry path"
        );
    }

    // ── format_compose_logs ────────────────────────────────

    #[test]
    fn test_format_compose_logs_basic() {
        let raw = "\
web-1  | 192.168.1.1 - GET / 200
web-1  | 192.168.1.1 - GET /favicon.ico 404
api-1  | Server listening on port 3000
api-1  | Connected to database";
        let out = format_compose_logs(raw);
        assert!(out.contains("Logs"), "should have compose logs header");
    }

    #[test]
    fn test_format_compose_logs_empty() {
        let out = format_compose_logs("");
        assert!(out.contains("No logs"), "should indicate no logs");
    }

    // ── format_compose_build ───────────────────────────────

    #[test]
    fn test_format_compose_build_basic() {
        let raw = "\
[+] Building 12.3s (8/8) FINISHED
 => [web internal] load build definition from Dockerfile           0.0s
 => [web internal] load metadata for docker.io/library/node:20     1.2s
 => [web 1/4] FROM docker.io/library/node:20@sha256:abc123         0.0s
 => [web 2/4] WORKDIR /app                                         0.1s
 => [web 3/4] COPY package*.json ./                                0.1s
 => [web 4/4] RUN npm install                                      8.5s
 => [web] exporting to image                                       2.3s
 => => naming to docker.io/library/myapp-web                       0.0s";
        let out = format_compose_build(raw);
        assert!(out.contains("12.3s"), "should show total build time");
        assert!(out.contains("web"), "should show service name");
        assert!(out.len() < raw.len(), "should be shorter than raw");
    }

    #[test]
    fn test_format_compose_build_empty() {
        let out = format_compose_build("");
        assert!(
            !out.is_empty(),
            "should produce output even for empty input"
        );
    }

    // ── compact_ports (existing, previously untested) ──────

    #[test]
    fn test_compact_ports_empty() {
        assert_eq!(compact_ports(""), "-");
    }

    #[test]
    fn test_compact_ports_single() {
        let result = compact_ports("0.0.0.0:8080->80/tcp");
        assert!(result.contains("8080"));
    }

    #[test]
    fn test_compact_ports_many() {
        let result = compact_ports("0.0.0.0:80->80/tcp, 0.0.0.0:443->443/tcp, 0.0.0.0:8080->8080/tcp, 0.0.0.0:9090->9090/tcp");
        assert!(result.contains("..."), "should truncate for >3 ports");
    }

    // ── format_docker_ps_line ─────────────────────────────

    /// Build the parts slice that `format_docker_ps_line` expects from named fields.
    fn ps_parts<'a>(
        id: &'a str,
        name: &'a str,
        status: &'a str,
        image: &'a str,
        ports: &'a str,
    ) -> Vec<&'a str> {
        vec![id, name, status, image, ports]
    }

    #[test]
    fn test_docker_ps_preserves_healthy_status() {
        let parts = ps_parts(
            "abc123def456",
            "my-api",
            "Up 3 hours (healthy)",
            "nginx:latest",
            "0.0.0.0:80->80/tcp",
        );
        let out = format_docker_ps_line(&parts).expect("should produce a line");

        assert!(
            out.contains("Up 3 hours (healthy)"),
            "healthy status should appear verbatim in output; got: {}",
            out
        );
        assert!(
            !out.contains("[!]"),
            "healthy container must NOT be flagged with [!]; got: {}",
            out
        );
    }

    #[test]
    fn test_docker_ps_marks_unhealthy_prominently() {
        let parts = ps_parts(
            "deadbeef1234",
            "broken-svc",
            "Up 1 hour (unhealthy)",
            "myapp:1.0",
            "",
        );
        let out = format_docker_ps_line(&parts).expect("should produce a line");

        assert!(
            out.contains("[!]"),
            "unhealthy container must be flagged with [!]; got: {}",
            out
        );
        assert!(
            out.contains("unhealthy"),
            "unhealthy keyword must still be present in output; got: {}",
            out
        );
    }

    #[test]
    fn test_docker_ps_shows_status_without_health_check() {
        let parts = ps_parts("cafe00112233", "simple-worker", "Up 5 minutes", "redis:7", "");
        let out = format_docker_ps_line(&parts).expect("should produce a line");

        assert!(
            out.contains("Up 5 minutes"),
            "plain status should appear in output; got: {}",
            out
        );
        assert!(
            !out.contains("[!]"),
            "container without health check must NOT be flagged with [!]; got: {}",
            out
        );
    }

    #[test]
    fn test_docker_ps_exited_no_flag() {
        // "Exited (1) 5 minutes ago" must NOT get a [!] prefix
        let parts = ps_parts(
            "1234567890ab",
            "crashed-job",
            "Exited (1) 5 minutes ago",
            "python:3.12",
            "",
        );
        let out = format_docker_ps_line(&parts).expect("should produce a line");

        assert!(
            out.contains("Exited (1) 5 minutes ago"),
            "exited status should appear verbatim; got: {}",
            out
        );
        assert!(
            !out.contains("[!]"),
            "Exited container must NOT be flagged with [!]; got: {}",
            out
        );
    }

    #[test]
    fn test_docker_ps_restarting_no_flag() {
        // "Restarting (0) 2 seconds ago" must NOT get a [!] prefix
        let parts = ps_parts(
            "abcdef012345",
            "flaky-svc",
            "Restarting (0) 2 seconds ago",
            "myapp:2.1",
            "",
        );
        let out = format_docker_ps_line(&parts).expect("should produce a line");

        assert!(
            out.contains("Restarting (0) 2 seconds ago"),
            "restarting status should appear verbatim; got: {}",
            out
        );
        assert!(
            !out.contains("[!]"),
            "Restarting container must NOT be flagged with [!]; got: {}",
            out
        );
    }

    #[test]
    fn test_docker_ps_line_too_few_parts_returns_none() {
        // Fewer than 4 fields → None (malformed line)
        let parts = vec!["abc123", "only-two-fields"];
        assert!(
            format_docker_ps_line(&parts).is_none(),
            "should return None for malformed lines with <4 fields"
        );
    }

    #[test]
    fn test_docker_ps_formats_all_five_containers() {
        // docker_ps() receives --format tab-separated output (already compact);
        // savings are measured at the docker images / kubectl level where output is larger.
        // This test verifies correct formatting of all container states.
        let cases = [
            ("abc123def45600", "web",    "Up 2 hours (healthy)",       "nginx:1.25",                      "0.0.0.0:80->80/tcp"),
            ("deadbeef123400", "api",    "Up 1 hour (unhealthy)",       "mycompany/backend-service:v2.3.1", "0.0.0.0:3000->3000/tcp"),
            ("cafe00112233",   "worker", "Up 5 minutes",                "python:3.12-slim",                ""),
            ("111122223333",   "db",     "Up 3 days",                   "postgres:16-alpine",               "0.0.0.0:5432->5432/tcp"),
            ("aaaabbbbcccc",   "cache",  "Exited (1) 10 minutes ago",   "redis:7",                         ""),
        ];

        for (id, name, status, image, ports) in &cases {
            let parts = ps_parts(id, name, status, image, ports);
            let out = format_docker_ps_line(&parts).expect("should produce a line");
            assert!(out.contains(name),   "name missing for {}: {}", name, out);
            assert!(out.contains(status) || out.contains("[!]"),
                "status missing for {}: {}", name, out);
        }

        // Unhealthy gets [!]; others do not
        let unhealthy = ps_parts("deadbeef123400", "api", "Up 1 hour (unhealthy)", "img:1", "");
        assert!(format_docker_ps_line(&unhealthy).unwrap().contains("[!]"));

        let healthy = ps_parts("abc123def456", "web", "Up 2 hours (healthy)", "img:1", "");
        assert!(!format_docker_ps_line(&healthy).unwrap().contains("[!]"));
    }
}
