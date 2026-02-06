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
