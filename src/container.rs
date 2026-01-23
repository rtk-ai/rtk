use anyhow::{Context, Result};
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
    let output = Command::new("docker")
        .args(["ps", "--format", "{{.Names}}\t{{.Status}}\t{{.Image}}\t{{.Ports}}"])
        .output()
        .context("Failed to run docker ps")?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    if stdout.trim().is_empty() {
        println!("🐳 0 containers");
        return Ok(());
    }

    let count = stdout.lines().count();
    println!("🐳 {} containers:", count);

    for line in stdout.lines().take(15) {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let name = parts[0];
            let image = parts.get(2).unwrap_or(&"");
            // Shorten image name
            let short_image = image.split('/').last().unwrap_or(image);
            let ports = compact_ports(parts.get(3).unwrap_or(&""));
            if ports == "-" {
                println!("  {} ({})", name, short_image);
            } else {
                println!("  {} ({}) [{}]", name, short_image, ports);
            }
        }
    }

    if count > 15 {
        println!("  ... +{} more", count - 15);
    }

    Ok(())
}

fn docker_images(_verbose: u8) -> Result<()> {
    let output = Command::new("docker")
        .args(["images", "--format", "{{.Repository}}:{{.Tag}}\t{{.Size}}"])
        .output()
        .context("Failed to run docker images")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();

    if lines.is_empty() {
        println!("🐳 0 images");
        return Ok(());
    }

    // Calculate total size
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

    println!("🐳 {} images ({})", lines.len(), total_display);

    for line in lines.iter().take(15) {
        let parts: Vec<&str> = line.split('\t').collect();
        if !parts.is_empty() {
            let image = parts[0];
            let size = parts.get(1).unwrap_or(&"");
            // Shorten image name
            let short = if image.len() > 40 {
                format!("...{}", &image[image.len()-37..])
            } else {
                image.to_string()
            };
            println!("  {} [{}]", short, size);
        }
    }

    if lines.len() > 15 {
        println!("  ... +{} more", lines.len() - 15);
    }

    Ok(())
}

fn docker_logs(args: &[String], _verbose: u8) -> Result<()> {
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
    let combined = format!("{}\n{}", stdout, stderr);

    // Use log deduplication
    let analyzed = crate::log_cmd::run_stdin_str(&combined);
    println!("🐳 Logs for {}:", container);
    println!("{}", analyzed);

    Ok(())
}

fn kubectl_pods(args: &[String], _verbose: u8) -> Result<()> {
    // Use JSON output for precise parsing
    let mut cmd = Command::new("kubectl");
    cmd.args(["get", "pods", "-o", "json"]);

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run kubectl get pods")?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON
    let json: serde_json::Value = match serde_json::from_str(&stdout) {
        Ok(v) => v,
        Err(_) => {
            println!("☸️  No pods found");
            return Ok(());
        }
    };

    let items = json["items"].as_array();
    if items.is_none() || items.unwrap().is_empty() {
        println!("☸️  No pods found");
        return Ok(());
    }

    let pods = items.unwrap();
    let mut running = 0;
    let mut pending = 0;
    let mut failed = 0;
    let mut restarts_total = 0;
    let mut issues: Vec<String> = Vec::new();

    for pod in pods {
        let ns = pod["metadata"]["namespace"].as_str().unwrap_or("-");
        let name = pod["metadata"]["name"].as_str().unwrap_or("-");
        let phase = pod["status"]["phase"].as_str().unwrap_or("Unknown");

        // Count restarts
        let mut pod_restarts = 0;
        if let Some(containers) = pod["status"]["containerStatuses"].as_array() {
            for c in containers {
                pod_restarts += c["restartCount"].as_i64().unwrap_or(0);
            }
        }
        restarts_total += pod_restarts;

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
                // Check for CrashLoopBackOff etc
                if let Some(containers) = pod["status"]["containerStatuses"].as_array() {
                    for c in containers {
                        if let Some(waiting) = c["state"]["waiting"]["reason"].as_str() {
                            if waiting.contains("CrashLoop") || waiting.contains("Error") {
                                failed += 1;
                                issues.push(format!("{}/{} {}", ns, name, waiting));
                            }
                        }
                    }
                }
            }
        }
    }

    // Summary line
    let total = pods.len();
    print!("☸️  {} pods: ", total);

    let mut parts = Vec::new();
    if running > 0 { parts.push(format!("{} ✓", running)); }
    if pending > 0 { parts.push(format!("{} pending", pending)); }
    if failed > 0 { parts.push(format!("{} ✗", failed)); }
    if restarts_total > 0 { parts.push(format!("{} restarts", restarts_total)); }

    println!("{}", parts.join(", "));

    // Show issues
    if !issues.is_empty() {
        println!("⚠️  Issues:");
        for issue in issues.iter().take(10) {
            println!("  {}", issue);
        }
        if issues.len() > 10 {
            println!("  ... +{} more", issues.len() - 10);
        }
    }

    Ok(())
}

fn kubectl_services(args: &[String], _verbose: u8) -> Result<()> {
    let mut cmd = Command::new("kubectl");
    cmd.args(["get", "services", "-o", "json"]);

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run kubectl get services")?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    let json: serde_json::Value = match serde_json::from_str(&stdout) {
        Ok(v) => v,
        Err(_) => {
            println!("☸️  No services found");
            return Ok(());
        }
    };

    let items = json["items"].as_array();
    if items.is_none() || items.unwrap().is_empty() {
        println!("☸️  No services found");
        return Ok(());
    }

    let services = items.unwrap();
    println!("☸️  {} services:", services.len());

    for svc in services.iter().take(15) {
        let ns = svc["metadata"]["namespace"].as_str().unwrap_or("-");
        let name = svc["metadata"]["name"].as_str().unwrap_or("-");
        let svc_type = svc["spec"]["type"].as_str().unwrap_or("-");

        // Extract ports
        let ports: Vec<String> = svc["spec"]["ports"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|p| {
                        let port = p["port"].as_i64().unwrap_or(0);
                        let target = p["targetPort"].as_i64()
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

        println!("  {}/{} {} [{}]", ns, name, svc_type, ports.join(","));
    }

    if services.len() > 15 {
        println!("  ... +{} more", services.len() - 15);
    }

    Ok(())
}

fn kubectl_logs(args: &[String], _verbose: u8) -> Result<()> {
    let pod = args.first().map(|s| s.as_str()).unwrap_or("");
    if pod.is_empty() {
        println!("Usage: rtk kubectl logs <pod>");
        return Ok(());
    }

    let mut cmd = Command::new("kubectl");
    cmd.args(["logs", "--tail", "100", pod]);

    // Add remaining args (like container name, -c, etc.)
    for arg in args.iter().skip(1) {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run kubectl logs")?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    let analyzed = crate::log_cmd::run_stdin_str(&stdout);
    println!("☸️  Logs for {}:", pod);
    println!("{}", analyzed);

    Ok(())
}

fn compact_ports(ports: &str) -> String {
    if ports.is_empty() {
        return "-".to_string();
    }

    // Extract just the port numbers
    let port_nums: Vec<&str> = ports
        .split(',')
        .filter_map(|p| {
            p.split("->")
                .next()
                .and_then(|s| s.split(':').last())
        })
        .collect();

    if port_nums.len() <= 3 {
        port_nums.join(", ")
    } else {
        format!("{}, ... +{}", port_nums[..2].join(", "), port_nums.len() - 2)
    }
}
