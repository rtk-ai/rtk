//! Filters Docker and kubectl output into compact summaries.

use crate::core::guard::never_worse;
use crate::core::runner::{self, RunOptions};
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::truncate::{CAP_INVENTORY, CAP_LIST, CAP_WARNINGS};
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use serde_json::Value;
use std::ffi::OsString;
use std::process::Command;

#[derive(Debug, Clone, Copy)]
pub enum ContainerCmd {
    DockerPs,
    DockerPsAll,
    DockerImages,
    DockerLogs,
    KubectlPods,
    KubectlServices,
    KubectlLogs,
}

pub fn run(cmd: ContainerCmd, args: &[String], verbose: u8) -> Result<i32> {
    match cmd {
        ContainerCmd::DockerPs => docker_ps(verbose),
        ContainerCmd::DockerPsAll => docker_ps_all(verbose),
        ContainerCmd::DockerImages => docker_images(verbose),
        ContainerCmd::DockerLogs => docker_logs(args, verbose),
        ContainerCmd::KubectlPods => k8s_pods("kubectl", args, verbose),
        ContainerCmd::KubectlServices => k8s_services("kubectl", args, verbose),
        ContainerCmd::KubectlLogs => k8s_logs("kubectl", args, verbose),
    }
}

fn run_k8s_json<F>(cmd: Command, tool: &str, label: &str, filter_fn: F) -> Result<i32>
where
    F: Fn(&Value) -> String,
{
    runner::run_filtered(
        cmd,
        tool,
        label,
        |stdout| match serde_json::from_str::<Value>(stdout) {
            Ok(json) => filter_fn(&json),
            Err(e) => {
                eprintln!("[rtk] {}: JSON parse failed: {}", tool, e);
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

    let base = exec_capture(resolved_command("docker").args(["ps"]))
        .context("Failed to run docker ps")?;
    if !base.success() {
        eprint!("{}", base.stderr);
        print!("{}", base.stdout);
        timer.track("docker ps", "rtk docker ps", &base.stdout, &base.stdout);
        return Ok(base.exit_code);
    }
    let raw = base.stdout;

    let stdout = match exec_capture(resolved_command("docker").args([
        "ps",
        "--format",
        "{{.ID}}\t{{.Names}}\t{{.Status}}\t{{.Image}}\t{{.Ports}}",
    ]))
    .ok()
    .filter(|r| r.success())
    {
        Some(r) => r.stdout,
        None => {
            print!("{}", raw);
            timer.track("docker ps", "rtk docker ps", &raw, &raw);
            return Ok(0);
        }
    };

    let mut rtk = String::new();

    const MAX_CONTAINERS: usize = CAP_LIST;
    let lines: Vec<String> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| format_container_line(line, true))
        .collect();

    rtk.push_str(&format!("[docker] {} containers:\n", lines.len()));
    for entry in lines.iter().take(MAX_CONTAINERS) {
        rtk.push_str(entry);
    }
    if lines.len() > MAX_CONTAINERS {
        rtk.push_str(&format!("  … +{} more\n", lines.len() - MAX_CONTAINERS));
        let full: String = lines.concat();
        if let Some(hint) = crate::core::tee::force_tee_hint(&full, "docker-ps") {
            rtk.push_str(&format!("{}\n", hint));
        }
    }

    let shown = never_worse(&raw, &rtk);
    print!("{}", shown);
    timer.track("docker ps", "rtk docker ps", &raw, shown);
    Ok(0)
}

fn docker_ps_all(_verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let base = exec_capture(resolved_command("docker").args(["ps", "-a"]))
        .context("Failed to run docker ps -a")?;
    if !base.success() {
        eprint!("{}", base.stderr);
        print!("{}", base.stdout);
        timer.track("docker ps -a", "rtk docker ps -a", &base.stdout, &base.stdout);
        return Ok(base.exit_code);
    }
    let raw = base.stdout;

    let stdout = match exec_capture(resolved_command("docker").args([
        "ps",
        "-a",
        "--format",
        "{{.State}}\t{{.ID}}\t{{.Names}}\t{{.Status}}\t{{.Image}}\t{{.Ports}}",
    ]))
    .ok()
    .filter(|r| r.success())
    {
        Some(r) => r.stdout,
        None => {
            print!("{}", raw);
            timer.track("docker ps -a", "rtk docker ps -a", &raw, &raw);
            return Ok(0);
        }
    };

    let mut running_lines: Vec<String> = Vec::new();
    let mut stopped_lines: Vec<String> = Vec::new();
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let parts: Vec<&str> = line.split('\t').collect();
        let state = parts.first().copied().unwrap_or("");
        let is_running = matches!(state, "running" | "restarting");
        if let Some(entry) = format_container_line_from_parts(&parts[1..], is_running) {
            if is_running {
                running_lines.push(entry);
            } else {
                stopped_lines.push(entry);
            }
        }
    }

    const MAX_CONTAINERS: usize = 20;
    let truncated = running_lines.len() > MAX_CONTAINERS || stopped_lines.len() > MAX_CONTAINERS;

    let mut rtk = String::new();
    rtk.push_str(&format!("[docker] {} running:\n", running_lines.len()));
    for l in running_lines.iter().take(MAX_CONTAINERS) {
        rtk.push_str(l);
    }
    if running_lines.len() > MAX_CONTAINERS {
        rtk.push_str(&format!(
            "  … +{} more\n",
            running_lines.len() - MAX_CONTAINERS
        ));
    }
    if !stopped_lines.is_empty() {
        rtk.push_str(&format!(
            "[docker] {} stopped/exited:\n",
            stopped_lines.len()
        ));
        for l in stopped_lines.iter().take(MAX_CONTAINERS) {
            rtk.push_str(l);
        }
        if stopped_lines.len() > MAX_CONTAINERS {
            rtk.push_str(&format!(
                "  … +{} more\n",
                stopped_lines.len() - MAX_CONTAINERS
            ));
        }
    }
    if truncated {
        let full: String = running_lines.iter().chain(stopped_lines.iter()).cloned().collect();
        if let Some(hint) = crate::core::tee::force_tee_hint(&full, "docker-ps-a") {
            rtk.push_str(&format!("{}\n", hint));
        }
    }

    let shown = never_worse(&raw, &rtk);
    print!("{}", shown);
    timer.track("docker ps -a", "rtk docker ps -a", &raw, shown);
    Ok(0)
}

fn format_container_line(line: &str, with_ports: bool) -> Option<String> {
    let parts: Vec<&str> = line.split('\t').collect();
    format_container_line_from_parts(&parts, with_ports)
}

fn format_container_line_from_parts(parts: &[&str], with_ports: bool) -> Option<String> {
    if parts.len() < 4 {
        return None;
    }
    let id = &parts[0][..12.min(parts[0].len())];
    let name = parts[1];
    let status = parts[2].trim();
    let short_image = parts[3].split('/').next_back().unwrap_or("");
    let port_suffix = if with_ports {
        let ports = compact_ports(parts.get(4).unwrap_or(&""));
        if ports == "-" {
            String::new()
        } else {
            format!(" [{}]", ports)
        }
    } else {
        String::new()
    };
    Some(format!(
        "  {} {} ({}) {}{}\n",
        id, name, short_image, status, port_suffix
    ))
}

fn docker_images(_verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let base = exec_capture(resolved_command("docker").args(["images"]))
        .context("Failed to run docker images")?;
    if !base.success() {
        eprint!("{}", base.stderr);
        print!("{}", base.stdout);
        timer.track("docker images", "rtk docker images", &base.stdout, &base.stdout);
        return Ok(base.exit_code);
    }
    let raw = base.stdout;

    let stdout = match exec_capture(resolved_command("docker").args([
        "images",
        "--format",
        "{{.Repository}}:{{.Tag}}\t{{.Size}}",
    ]))
    .ok()
    .filter(|r| r.success())
    {
        Some(r) => r.stdout,
        None => {
            print!("{}", raw);
            timer.track("docker images", "rtk docker images", &raw, &raw);
            return Ok(0);
        }
    };

    let lines: Vec<&str> = stdout.lines().collect();
    let mut rtk = String::new();

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

    // a full image list is an inventory query, like pip list.
    const MAX_IMAGES: usize = CAP_INVENTORY;
    let image_lines: Vec<String> = lines
        .iter()
        .map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            let image = parts.first().copied().unwrap_or("");
            let size = parts.get(1).copied().unwrap_or("");
            format!("  {} [{}]\n", image, size)
        })
        .collect();

    let mut full_rtk = rtk.clone();
    for l in &image_lines {
        full_rtk.push_str(l);
    }

    for l in image_lines.iter().take(MAX_IMAGES) {
        rtk.push_str(l);
    }
    if image_lines.len() > MAX_IMAGES {
        rtk.push_str(&format!("  … +{} more\n", image_lines.len() - MAX_IMAGES));
        if let Some(hint) = crate::core::tee::force_tee_tail_hint(&full_rtk, "docker-images", MAX_IMAGES + 2) {
            rtk.push_str(&format!("{}\n", hint));
        }
    }

    let shown = never_worse(&raw, &rtk);
    print!("{}", shown);
    timer.track("docker images", "rtk docker images", &raw, shown);
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

pub fn k8s_pods(tool: &str, args: &[String], _verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command(tool);
    cmd.args(["get", "pods", "-o", "json"]);
    for arg in args {
        cmd.arg(arg);
    }
    run_k8s_json(cmd, tool, "get pods", format_kubectl_pods)
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
        const MAX_PODS_ISSUES: usize = CAP_WARNINGS;
        out.push_str("[warn] Issues:\n");
        for issue in issues.iter().take(MAX_PODS_ISSUES) {
            out.push_str(&format!("  {}\n", issue));
        }
        if issues.len() > MAX_PODS_ISSUES {
            out.push_str(&format!("  … +{} more", issues.len() - MAX_PODS_ISSUES));
            let all_issues = issues.join("\n");
            if let Some(hint) =
                crate::core::tee::force_tee_tail_hint(&all_issues, "kubectl-pods", MAX_PODS_ISSUES + 1)
            {
                out.push_str(&format!(" {}", hint));
            }
        }
    }
    out
}

pub fn k8s_services(tool: &str, args: &[String], _verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command(tool);
    cmd.args(["get", "services", "-o", "json"]);
    for arg in args {
        cmd.arg(arg);
    }
    run_k8s_json(cmd, tool, "get services", format_kubectl_services)
}

fn format_kubectl_services(json: &Value) -> String {
    let Some(services) = json["items"].as_array().filter(|a| !a.is_empty()) else {
        return "No services found\n".to_string();
    };
    let mut out = format!("{} services:\n", services.len());

    let all_lines: Vec<String> = services
        .iter()
        .map(|svc| {
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
            format!("  {}/{} {} [{}]", ns, name, svc_type, ports.join(","))
        })
        .collect();

    const MAX_KUBECTL_SERVICES: usize = CAP_LIST;
    for line in all_lines.iter().take(MAX_KUBECTL_SERVICES) {
        out.push_str(&format!("{}\n", line));
    }
    if all_lines.len() > MAX_KUBECTL_SERVICES {
        out.push_str(&format!("  … +{} more", all_lines.len() - MAX_KUBECTL_SERVICES));
        let all_text = all_lines.join("\n");
        if let Some(hint) =
            crate::core::tee::force_tee_tail_hint(&all_text, "kubectl-services", MAX_KUBECTL_SERVICES + 1)
        {
            out.push_str(&format!(" {}", hint));
        }
        out.push('\n');
    }
    out
}

pub fn k8s_logs(tool: &str, args: &[String], _verbose: u8) -> Result<i32> {
    let pod = args.first().map(|s| s.as_str()).unwrap_or("");
    if pod.is_empty() {
        println!("Usage: rtk {} logs <pod>", tool);
        return Ok(0);
    }

    let mut cmd = resolved_command(tool);
    cmd.args(["logs", "--tail", "100", pod]);
    for arg in args.iter().skip(1) {
        cmd.arg(arg);
    }

    let label = format!("logs {}", pod);
    runner::run_filtered(
        cmd,
        tool,
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
    const MAX_COMPOSE_SERVICES: usize = CAP_LIST;
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();

    if lines.is_empty() {
        return "[compose] 0 services".to_string();
    }

    let mut result = format!("[compose] {} services:\n", lines.len());

    // Pre-build all formatted lines so the tee file matches what the agent sees.
    let all_formatted: Vec<String> = lines
        .iter()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 4 {
                return None;
            }
            let name = parts[0];
            let image = parts[1];
            let status = parts[2];
            let ports = parts[3];
            let short_image = image.split('/').next_back().unwrap_or(image);
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
            Some(format!("  {} ({}) {}{}", name, short_image, status, port_str))
        })
        .collect();

    for line in all_formatted.iter().take(MAX_COMPOSE_SERVICES) {
        result.push_str(line);
        result.push('\n');
    }
    if all_formatted.len() > MAX_COMPOSE_SERVICES {
        result.push_str(&format!("  … +{} more\n", all_formatted.len() - MAX_COMPOSE_SERVICES));
        let all_text = all_formatted.join("\n");
        if let Some(hint) = crate::core::tee::force_tee_tail_hint(&all_text, "compose-ps", MAX_COMPOSE_SERVICES + 1) {
            result.push_str(&format!("  {}\n", hint));
        }
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

/// Format `docker compose up -d` output into a compact summary.
///
/// Compose reprints each network/container as it moves through status
/// transitions (e.g. Created → Starting → Started, or Running → Waiting →
/// Healthy for entities with healthchecks) — keep only the last reported
/// status per entity instead of every transition line.
pub fn format_compose_up(raw: &str) -> String {
    if raw.trim().is_empty() {
        return "[compose] up: no output".to_string();
    }

    let mut order: Vec<String> = Vec::new();
    let mut entities: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    let mut orphan_warning = false;
    let mut other_warnings = 0usize;

    for line in raw.lines() {
        let trimmed = line.trim().trim_start_matches(['✔', '✘']).trim();
        if trimmed.is_empty() {
            continue;
        }

        // Docker's own log lines (orphan container notices, etc.) rather than
        // a compose status line — surface as a condensed warning, not verbatim.
        if trimmed.starts_with("time=") {
            if trimmed.contains("orphan containers") {
                orphan_warning = true;
            } else if trimmed.contains("level=warning") || trimmed.contains("level=error") {
                other_warnings += 1;
            }
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 3 || !matches!(parts[0], "Network" | "Volume" | "Container" | "Image") {
            continue;
        }
        let kind = parts[0].to_lowercase();
        let name = parts[1].to_string();
        let status = parts[2].to_string();

        if !entities.contains_key(&name) {
            order.push(name.clone());
        }
        entities.insert(name, (kind, status));
    }

    if order.is_empty() {
        return "[compose] up: no output".to_string();
    }

    // Group by final status (most entities converge on the same one or two
    // statuses) instead of repeating a line per entity.
    let mut status_order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for name in &order {
        let (kind, status) = entities.get(name).expect("name was just pushed to order");
        let label = if kind == "container" {
            name.clone()
        } else {
            format!("{} ({})", name, kind)
        };
        groups.entry(status.clone()).or_insert_with(|| {
            status_order.push(status.clone());
            Vec::new()
        });
        groups.get_mut(status).unwrap().push(label);
    }

    let mut result = format!("[compose] up: {} services\n", order.len());
    for status in &status_order {
        let names = groups.get(status).expect("status was just recorded");
        result.push_str(&format!("  {}: {}\n", status, names.join(", ")));
    }
    if orphan_warning {
        result.push_str("[warn] orphan containers — run with --remove-orphans\n");
    }
    if other_warnings > 0 {
        result.push_str(&format!("[warn] {} other warning(s)\n", other_warnings));
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
        .filter_map(|p| p.split("->").next().and_then(|s| s.split(':').next_back()))
        .collect();

    if port_nums.len() <= 3 {
        port_nums.join(", ")
    } else {
        format!(
            "{}, … +{}",
            port_nums[..2].join(", "),
            port_nums.len() - 2
        )
    }
}

pub fn run_docker_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    crate::core::runner::run_passthrough("docker", args, verbose)
}

/// Run `docker compose ps` (or `docker compose ps -a`) with compact output
pub fn run_compose_ps(all: bool, verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut raw_args: Vec<&str> = vec!["compose", "ps"];
    if all {
        raw_args.push("-a");
    }
    let raw_result = exec_capture(resolved_command("docker").args(&raw_args))
        .context("Failed to run docker compose ps")?;

    if !raw_result.success() {
        eprintln!("{}", raw_result.stderr);
        return Ok(raw_result.exit_code);
    }
    let raw = raw_result.stdout;

    let mut format_args: Vec<&str> = vec!["compose", "ps"];
    if all {
        format_args.push("-a");
    }
    format_args.extend(["--format", "{{.Name}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}"]);
    let result = exec_capture(resolved_command("docker").args(&format_args))
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
    let shown = never_worse(&raw, &rtk);
    println!("{}", shown);
    let label = if all { "docker compose ps -a" } else { "docker compose ps" };
    let rtk_label = if all { "rtk docker compose ps -a" } else { "rtk docker compose ps" };
    timer.track(label, rtk_label, &raw, shown);
    Ok(0)
}

pub fn run_compose_logs(service: Option<&str>, tail: u32, verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("docker");
    let tail_str = tail.to_string();
    cmd.args(["compose", "logs", "--tail", &tail_str]);
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

/// Run `docker compose up`. Only detached runs (`-d`/`--detach`) are filtered
/// into a compact summary — a foreground `up` streams logs indefinitely, so it
/// passes through untouched rather than buffering forever waiting for exit.
pub fn run_compose_up(args: &[String], verbose: u8) -> Result<i32> {
    let detached = args.iter().any(|a| a == "-d" || a == "--detach");
    if !detached {
        let mut combined = vec![OsString::from("compose"), OsString::from("up")];
        combined.extend(args.iter().map(OsString::from));
        return crate::core::runner::run_passthrough("docker", &combined, verbose);
    }

    let mut cmd = resolved_command("docker");
    cmd.arg("compose").arg("up");
    for a in args {
        cmd.arg(a);
    }

    runner::run_filtered(
        cmd,
        "docker",
        "compose up",
        |raw| {
            if verbose > 0 {
                eprintln!("raw docker compose up:\n{}", raw);
            }
            format_compose_up(raw)
        },
        RunOptions::default().early_exit_on_failure(),
    )
}

pub fn run_compose_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    let mut combined = vec![OsString::from("compose")];
    combined.extend_from_slice(args);
    crate::core::runner::run_passthrough("docker", &combined, verbose)
}

pub fn run_kubectl_get(args: &[String], verbose: u8) -> Result<i32> {
    run_k8s_get("kubectl", args, verbose)
}

fn run_k8s_get(tool: &str, args: &[String], verbose: u8) -> Result<i32> {
    match k8s_get_target(args) {
        Some(("pods", rest)) => k8s_pods(tool, rest, verbose),
        Some(("services", rest)) => k8s_services(tool, rest, verbose),
        _ => {
            let passthrough_args: Vec<OsString> = std::iter::once(OsString::from("get"))
                .chain(args.iter().map(|arg| OsString::from(arg.as_str())))
                .collect();
            crate::core::runner::run_passthrough(tool, &passthrough_args, verbose)
        }
    }
}

fn k8s_get_target(args: &[String]) -> Option<(&'static str, &[String])> {
    let resource = args.first()?.as_str();
    let rest = &args[1..];
    if k8s_get_requests_raw_output(rest) {
        return None;
    }

    match resource {
        "po" | "pod" | "pods" => Some(("pods", rest)),
        "svc" | "service" | "services" => Some(("services", rest)),
        _ => None,
    }
}

fn k8s_get_requests_raw_output(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "-o" | "--output" | "-w" | "--watch" | "--show-labels" | "--show-kind"
        ) || arg.starts_with("-o")
            || arg.starts_with("--output=")
    })
}

pub fn run_kubectl_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    crate::core::runner::run_passthrough("kubectl", args, verbose)
}

pub fn run_oc_get(args: &[String], verbose: u8) -> Result<i32> {
    run_k8s_get("oc", args, verbose)
}

pub fn run_oc_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    crate::core::runner::run_passthrough("oc", args, verbose)
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

    // ── format_compose_up ───────────────────────────────────

    #[test]
    fn test_format_compose_up_real_fixture_savings() {
        let raw = include_str!("../../../tests/fixtures/docker_compose_up_raw.txt");
        let out = format_compose_up(raw);

        assert!(out.contains("4 services"), "should count distinct entities");
        assert!(out.contains("datalake-postgres-1"), "should list container name");
        // postgres transitions Running -> Waiting -> Healthy; only the last should show.
        assert!(out.contains("Healthy"), "should keep the final status per entity");
        assert_eq!(
            out.matches("datalake-postgres-1").count(),
            1,
            "should not repeat an entity that transitioned through several statuses"
        );
        assert!(out.contains("[warn] orphan containers"), "should surface the orphan warning");
        assert!(
            !out.contains("level=warning"),
            "should not leak raw docker log formatting"
        );

        let input_tokens = raw.split_whitespace().count();
        let output_tokens = out.split_whitespace().count();
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(savings >= 60.0, "Expected >=60% savings, got {:.1}%", savings);
    }

    #[test]
    fn test_format_compose_up_fresh_creation() {
        // Compose prints a distinct set of verbs when creating from scratch
        // rather than reconciling already-running containers. The network is
        // only ever "Created" (never "Started"), so it must be kept — only the
        // containers' intermediate "Starting" transition should be dropped.
        let raw = "\
 Network myapp_default  Created
 Container myapp-db-1  Created
 Container myapp-web-1  Created
 Container myapp-db-1  Starting
 Container myapp-web-1  Starting
 Container myapp-db-1  Started
 Container myapp-web-1  Started";
        let out = format_compose_up(raw);
        assert!(out.contains("3 services"), "should count network + 2 containers");
        assert!(out.contains("myapp-db-1"), "should show container name");
        assert!(out.contains("myapp_default"), "should show network name");
        assert!(out.contains("network"), "should label the non-container entity");
        assert!(out.contains("Started"), "should keep the containers' final status");
        assert!(
            !out.contains("Starting"),
            "intermediate container transitions should be dropped"
        );
    }

    #[test]
    fn test_format_compose_up_empty() {
        let out = format_compose_up("");
        assert!(out.contains("no output"));
    }

    #[test]
    fn test_format_compose_up_whitespace_only() {
        let out = format_compose_up("   \n  \n");
        assert!(out.contains("no output"));
    }

    #[test]
    fn test_format_compose_up_no_orphan_warning_when_absent() {
        let raw = " Container myapp-web-1 Running";
        let out = format_compose_up(raw);
        assert!(
            !out.contains("[warn]"),
            "should not mention orphans when there is no such warning"
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
        assert!(result.contains("…"), "should truncate for >3 ports");
    }

    #[test]
    fn test_k8s_get_target_pods_aliases() {
        for resource in ["po", "pod", "pods"] {
            let args = vec![resource.to_string(), "-n".to_string(), "default".to_string()];

            assert_eq!(
                k8s_get_target(&args),
                Some(("pods", &args[1..])),
                "failed for {resource}"
            );
        }
    }

    #[test]
    fn test_k8s_get_target_services_aliases() {
        for resource in ["svc", "service", "services"] {
            let args = vec![resource.to_string(), "-A".to_string()];

            assert_eq!(
                k8s_get_target(&args),
                Some(("services", &args[1..])),
                "failed for {resource}"
            );
        }
    }

    #[test]
    fn test_k8s_get_target_unsupported_resource() {
        let args = vec!["deployments".to_string()];

        assert_eq!(k8s_get_target(&args), None);
    }

    #[test]
    fn test_k8s_get_target_respects_output_flags() {
        for output_flag in ["-o", "-owide", "--output", "--output=json"] {
            let args = vec![
                "pods".to_string(),
                output_flag.to_string(),
                "wide".to_string(),
            ];

            assert_eq!(
                k8s_get_target(&args),
                None,
                "should pass through {output_flag}"
            );
        }
    }

    // ── oc support ────────────────────────────────────────

    #[test]
    fn test_oc_pods_savings() {
        let input_str = include_str!("../../../tests/fixtures/oc_pods.json");
        let input: Value = serde_json::from_str(input_str).expect("fixture should parse");
        let output = format_kubectl_pods(&input);
        let input_tokens = input_str.split_whitespace().count();
        let output_tokens = output.split_whitespace().count();
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expected >=60% savings, got {:.1}%",
            savings
        );
    }
}
