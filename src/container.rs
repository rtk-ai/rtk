use crate::tracking;
use anyhow::{Context, Result};
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

pub fn run(cmd: ContainerCmd, args: &[String], verbose: u8) -> Result<()> {
    match cmd {
        ContainerCmd::DockerPs => docker_ps(verbose),
        ContainerCmd::DockerImages => docker_images(verbose),
        ContainerCmd::DockerLogs => docker_logs(args, verbose),
        ContainerCmd::KubectlPods => kubectl_pods(args, verbose),
        ContainerCmd::KubectlServices => kubectl_services(args, verbose),
        ContainerCmd::KubectlLogs => kubectl_logs(args, verbose),
    }
}

fn docker_ps(_verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let raw = Command::new("docker")
        .args(["ps"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let output = Command::new("docker")
        .args([
            "ps",
            "--format",
            "{{.ID}}\t{{.Names}}\t{{.Status}}\t{{.Image}}\t{{.Ports}}",
        ])
        .output()
        .context("Failed to run docker ps")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut rtk = String::new();

    if stdout.trim().is_empty() {
        rtk.push_str("🐳 0 containers");
        println!("{}", rtk);
        timer.track("docker ps", "rtk docker ps", &raw, &rtk);
        return Ok(());
    }

    let count = stdout.lines().count();
    rtk.push_str(&format!("🐳 {} containers:\n", count));

    for line in stdout.lines().take(15) {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 4 {
            let id = &parts[0][..12.min(parts[0].len())];
            let name = parts[1];
            let short_image = parts.get(3).unwrap_or(&"").split('/').last().unwrap_or("");
            let ports = compact_ports(parts.get(4).unwrap_or(&""));
            if ports == "-" {
                rtk.push_str(&format!("  {} {} ({})\n", id, name, short_image));
            } else {
                rtk.push_str(&format!(
                    "  {} {} ({}) [{}]\n",
                    id, name, short_image, ports
                ));
            }
        }
    }
    if count > 15 {
        rtk.push_str(&format!("  ... +{} more", count - 15));
    }

    print!("{}", rtk);
    timer.track("docker ps", "rtk docker ps", &raw, &rtk);
    Ok(())
}

fn docker_images(_verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let raw = Command::new("docker")
        .args(["images"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let output = Command::new("docker")
        .args(["images", "--format", "{{.Repository}}:{{.Tag}}\t{{.Size}}"])
        .output()
        .context("Failed to run docker images")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    let mut rtk = String::new();

    if lines.is_empty() {
        rtk.push_str("🐳 0 images");
        println!("{}", rtk);
        timer.track("docker images", "rtk docker images", &raw, &rtk);
        return Ok(());
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
    rtk.push_str(&format!("🐳 {} images ({})\n", lines.len(), total_display));

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
    Ok(())
}

fn docker_logs(args: &[String], _verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let container = args.first().map(|s| s.as_str()).unwrap_or("");
    if container.is_empty() {
        println!("Usage: rtk docker logs <container>");
        return Ok(());
    }

    let output = Command::new("docker")
        .args(["logs", "--tail", "100", container])
        .output()
        .context("Failed to run docker logs")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let analyzed = crate::log_cmd::run_stdin_str(&raw);
    let rtk = format!("🐳 Logs for {}:\n{}", container, analyzed);
    println!("{}", rtk);
    timer.track(
        &format!("docker logs {}", container),
        "rtk docker logs",
        &raw,
        &rtk,
    );
    Ok(())
}

fn kubectl_pods(args: &[String], _verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("kubectl");
    cmd.args(["get", "pods", "-o", "json"]);
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run kubectl get pods")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let mut rtk = String::new();

    let json: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            rtk.push_str("☸️  No pods found");
            println!("{}", rtk);
            timer.track("kubectl get pods", "rtk kubectl pods", &raw, &rtk);
            return Ok(());
        }
    };

    let items = json["items"].as_array();
    if items.is_none() || items.unwrap().is_empty() {
        rtk.push_str("☸️  No pods found");
        println!("{}", rtk);
        timer.track("kubectl get pods", "rtk kubectl pods", &raw, &rtk);
        return Ok(());
    }

    let pods = items.unwrap();
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
        parts.push(format!("{} ✓", running));
    }
    if pending > 0 {
        parts.push(format!("{} pending", pending));
    }
    if failed > 0 {
        parts.push(format!("{} ✗", failed));
    }
    if restarts_total > 0 {
        parts.push(format!("{} restarts", restarts_total));
    }

    rtk.push_str(&format!("☸️  {} pods: {}\n", pods.len(), parts.join(", ")));
    if !issues.is_empty() {
        rtk.push_str("⚠️  Issues:\n");
        for issue in issues.iter().take(10) {
            rtk.push_str(&format!("  {}\n", issue));
        }
        if issues.len() > 10 {
            rtk.push_str(&format!("  ... +{} more", issues.len() - 10));
        }
    }

    print!("{}", rtk);
    timer.track("kubectl get pods", "rtk kubectl pods", &raw, &rtk);
    Ok(())
}

fn kubectl_services(args: &[String], _verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("kubectl");
    cmd.args(["get", "services", "-o", "json"]);
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run kubectl get services")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let mut rtk = String::new();

    let json: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            rtk.push_str("☸️  No services found");
            println!("{}", rtk);
            timer.track("kubectl get svc", "rtk kubectl svc", &raw, &rtk);
            return Ok(());
        }
    };

    let items = json["items"].as_array();
    if items.is_none() || items.unwrap().is_empty() {
        rtk.push_str("☸️  No services found");
        println!("{}", rtk);
        timer.track("kubectl get svc", "rtk kubectl svc", &raw, &rtk);
        return Ok(());
    }

    let services = items.unwrap();
    rtk.push_str(&format!("☸️  {} services:\n", services.len()));

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
        rtk.push_str(&format!(
            "  {}/{} {} [{}]\n",
            ns,
            name,
            svc_type,
            ports.join(",")
        ));
    }
    if services.len() > 15 {
        rtk.push_str(&format!("  ... +{} more", services.len() - 15));
    }

    print!("{}", rtk);
    timer.track("kubectl get svc", "rtk kubectl svc", &raw, &rtk);
    Ok(())
}

fn kubectl_logs(args: &[String], _verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let pod = args.first().map(|s| s.as_str()).unwrap_or("");
    if pod.is_empty() {
        println!("Usage: rtk kubectl logs <pod>");
        return Ok(());
    }

    let mut cmd = Command::new("kubectl");
    cmd.args(["logs", "--tail", "100", pod]);
    for arg in args.iter().skip(1) {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run kubectl logs")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let analyzed = crate::log_cmd::run_stdin_str(&raw);
    let rtk = format!("☸️  Logs for {}:\n{}", pod, analyzed);
    println!("{}", rtk);
    timer.track(
        &format!("kubectl logs {}", pod),
        "rtk kubectl logs",
        &raw,
        &rtk,
    );
    Ok(())
}

/// Format `docker compose ps` output into compact form
pub fn format_compose_ps(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();

    // Empty or header-only
    if lines.len() <= 1 {
        return "🐳 0 compose services".to_string();
    }

    // Skip the header row
    let services: Vec<&str> = lines[1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .copied()
        .collect();
    if services.is_empty() {
        return "🐳 0 compose services".to_string();
    }

    let mut result = format!("🐳 {} compose services:\n", services.len());

    for line in services.iter().take(20) {
        // docker compose ps columns are whitespace-separated but STATUS can contain spaces
        // Columns: NAME IMAGE COMMAND SERVICE CREATED STATUS PORTS
        // We split by 2+ spaces to handle multi-word fields
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 6 {
            let name = parts[0];
            let image = parts[1];

            // Find SERVICE column (usually index 3, after quoted COMMAND)
            // Find STATUS by looking for "Up" or "Exited" keywords
            let line_str = *line;
            let status = if line_str.contains("Up") {
                let idx = line_str.find("Up").unwrap();
                let end = line_str[idx..]
                    .find("   ")
                    .map(|e| idx + e)
                    .unwrap_or(line_str.len());
                line_str[idx..end].trim()
            } else if line_str.contains("Exited") {
                let idx = line_str.find("Exited").unwrap();
                let end = line_str[idx..]
                    .find("   ")
                    .map(|e| idx + e)
                    .unwrap_or(line_str.len());
                line_str[idx..end].trim()
            } else {
                "unknown"
            };

            // Extract port from end of line
            let port_str = if let Some(port_idx) = line_str.rfind("0.0.0.0:") {
                let chars: Vec<char> = line_str.chars().collect();
                let port_chars: Vec<char> = chars[port_idx..].to_vec();
                let port_text: String = port_chars.iter().collect();
                let compact = compact_ports(port_text.trim());
                format!(" [{}]", compact)
            } else {
                String::new()
            };

            // Shorten image name: keep only the last segment
            let short_image = image.split('/').last().unwrap_or(image);

            result.push_str(&format!(
                "  {} ({}) {}{}\n",
                name, short_image, status, port_str
            ));
        }
    }
    if services.len() > 20 {
        result.push_str(&format!("  ... +{} more\n", services.len() - 20));
    }

    result.trim_end().to_string()
}

/// Format `docker compose logs` output into compact form
pub fn format_compose_logs(raw: &str) -> String {
    if raw.trim().is_empty() {
        return "🐳 No logs".to_string();
    }

    // docker compose logs prefixes each line with "service-N  | "
    // Use the existing log deduplication engine
    let analyzed = crate::log_cmd::run_stdin_str(raw);
    format!("🐳 Compose logs:\n{}", analyzed)
}

/// Format `docker compose build` output into compact summary
pub fn format_compose_build(raw: &str) -> String {
    if raw.trim().is_empty() {
        return "🐳 Build: no output".to_string();
    }

    let mut result = String::new();

    // Extract the summary line: "[+] Building 12.3s (8/8) FINISHED"
    for line in raw.lines() {
        if line.contains("Building") && line.contains("FINISHED") {
            result.push_str(&format!("🐳 {}\n", line.trim()));
            break;
        }
    }

    if result.is_empty() {
        // No FINISHED line found — might still be building or errored
        if let Some(line) = raw.lines().find(|l| l.contains("Building")) {
            result.push_str(&format!("🐳 {}\n", line.trim()));
        } else {
            result.push_str("🐳 Build:\n");
        }
    }

    // Collect unique service names from build steps like "[web 1/4]"
    let mut services: Vec<String> = Vec::new();
    for line in raw.lines() {
        if let Some(start) = line.find('[') {
            let after: String = line.chars().skip(start + 1).collect();
            if let Some(end) = after.find(']') {
                let bracket: String = after.chars().take(end).collect();
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
    let steps: Vec<&str> = raw
        .lines()
        .filter(|l| l.trim_start().starts_with("=> "))
        .collect();
    if !steps.is_empty() {
        result.push_str(&format!("  Steps: {}", steps.len()));
    }

    result.trim_end().to_string()
}

fn compact_ports(ports: &str) -> String {
    if ports.is_empty() {
        return "-".to_string();
    }

    // Extract just the port numbers
    let port_nums: Vec<&str> = ports
        .split(',')
        .filter_map(|p| p.split("->").next().and_then(|s| s.split(':').last()))
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

/// Runs an unsupported docker subcommand by passing it through directly
pub fn run_docker_passthrough(args: &[OsString], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("docker passthrough: {:?}", args);
    }
    let status = Command::new("docker")
        .args(args)
        .status()
        .context("Failed to run docker")?;

    let args_str = tracking::args_display(args);
    timer.track_passthrough(
        &format!("docker {}", args_str),
        &format!("rtk docker {} (passthrough)", args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// Run `docker compose ps` with compact output
pub fn run_compose_ps(verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let raw = Command::new("docker")
        .args(["compose", "ps"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    if verbose > 0 {
        eprintln!("raw docker compose ps:\n{}", raw);
    }

    let rtk = format_compose_ps(&raw);
    println!("{}", rtk);
    timer.track("docker compose ps", "rtk docker compose ps", &raw, &rtk);
    Ok(())
}

/// Run `docker compose logs` with deduplication
pub fn run_compose_logs(service: Option<&str>, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("docker");
    cmd.args(["compose", "logs", "--tail", "100"]);
    if let Some(svc) = service {
        cmd.arg(svc);
    }

    let output = cmd.output().context("Failed to run docker compose logs")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    if verbose > 0 {
        eprintln!("raw docker compose logs:\n{}", raw);
    }

    let rtk = format_compose_logs(&raw);
    println!("{}", rtk);
    let svc_label = service.unwrap_or("all");
    timer.track(
        &format!("docker compose logs {}", svc_label),
        "rtk docker compose logs",
        &raw,
        &rtk,
    );
    Ok(())
}

/// Run `docker compose build` with summary output
pub fn run_compose_build(service: Option<&str>, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("docker");
    cmd.args(["compose", "build"]);
    if let Some(svc) = service {
        cmd.arg(svc);
    }

    let output = cmd.output().context("Failed to run docker compose build")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    if verbose > 0 {
        eprintln!("raw docker compose build:\n{}", raw);
    }

    let rtk = format_compose_build(&raw);
    println!("{}", rtk);
    let svc_label = service.unwrap_or("all");
    timer.track(
        &format!("docker compose build {}", svc_label),
        "rtk docker compose build",
        &raw,
        &rtk,
    );
    Ok(())
}

/// Runs an unsupported docker compose subcommand by passing it through directly
pub fn run_compose_passthrough(args: &[OsString], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("docker compose passthrough: {:?}", args);
    }
    let status = Command::new("docker")
        .arg("compose")
        .args(args)
        .status()
        .context("Failed to run docker compose")?;

    let args_str = tracking::args_display(args);
    timer.track_passthrough(
        &format!("docker compose {}", args_str),
        &format!("rtk docker compose {} (passthrough)", args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// Runs an unsupported kubectl subcommand by passing it through directly
pub fn run_kubectl_passthrough(args: &[OsString], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("kubectl passthrough: {:?}", args);
    }
    let status = Command::new("kubectl")
        .args(args)
        .status()
        .context("Failed to run kubectl")?;

    let args_str = tracking::args_display(args);
    timer.track_passthrough(
        &format!("kubectl {}", args_str),
        &format!("rtk kubectl {} (passthrough)", args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── format_compose_ps ──────────────────────────────────

    #[test]
    fn test_format_compose_ps_basic() {
        let raw = "\
NAME        IMAGE          COMMAND                  SERVICE   CREATED        STATUS         PORTS
web-1       nginx:latest   \"/docker-entrypoint.…\"   web       2 hours ago    Up 2 hours     0.0.0.0:80->80/tcp
api-1       node:20        \"docker-entrypoint.s…\"   api       2 hours ago    Up 2 hours     0.0.0.0:3000->3000/tcp
db-1        postgres:16    \"docker-entrypoint.s…\"   db        2 hours ago    Up 2 hours     0.0.0.0:5432->5432/tcp";
        let out = format_compose_ps(raw);
        assert!(out.contains("3"), "should show container count");
        assert!(out.contains("web"), "should show service name");
        assert!(out.contains("api"), "should show service name");
        assert!(out.contains("db"), "should show service name");
        assert!(out.len() < raw.len(), "output should be shorter than raw");
    }

    #[test]
    fn test_format_compose_ps_empty() {
        let out = format_compose_ps("");
        assert!(out.contains("0"), "should show zero containers");
    }

    #[test]
    fn test_format_compose_ps_header_only() {
        let raw = "NAME   IMAGE   COMMAND   SERVICE   CREATED   STATUS   PORTS";
        let out = format_compose_ps(raw);
        assert!(out.contains("0"), "should show zero containers");
    }

    #[test]
    fn test_format_compose_ps_exited_service() {
        let raw = "\
NAME        IMAGE          COMMAND              SERVICE   CREATED      STATUS                     PORTS
worker-1    python:3.12    \"python worker.py\"   worker    5 min ago    Exited (1) 2 minutes ago   ";
        let out = format_compose_ps(raw);
        assert!(out.contains("worker"), "should show service name");
        assert!(out.contains("Exited"), "should show exited status");
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
        assert!(!out.is_empty(), "should produce output");
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
}
