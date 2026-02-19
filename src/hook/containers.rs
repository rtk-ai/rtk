use super::helpers::replace_prefix;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    /// Strips docker global flags: -H, --context, --config
    static ref DOCKER_GLOBAL_FLAGS_RE: Regex =
        Regex::new(r"(?:(-H|--context|--config)\s+\S+\s*|--[a-z-]+=\S+\s*)").unwrap();

    /// Strips kubectl global flags: --context, --kubeconfig, --namespace, -n
    static ref KUBECTL_GLOBAL_FLAGS_RE: Regex =
        Regex::new(r"(?:(--context|--kubeconfig|--namespace|-n)\s+\S+\s*|--[a-z-]+=\S+\s*)").unwrap();
}

pub fn try_rewrite_docker(match_cmd: &str, cmd_body: &str) -> Option<String> {
    let after_docker = match_cmd.strip_prefix("docker ").unwrap_or("");

    // docker compose → always rewrite
    if after_docker.starts_with("compose") {
        return Some(replace_prefix(cmd_body, "docker ", "rtk docker "));
    }

    // Strip docker global flags
    let stripped = DOCKER_GLOBAL_FLAGS_RE.replace_all(after_docker, "");
    let stripped = stripped.trim_start();
    let subcmd = stripped.split_whitespace().next().unwrap_or("");
    match subcmd {
        "ps" | "images" | "logs" | "run" | "build" | "exec" => {
            Some(replace_prefix(cmd_body, "docker ", "rtk docker "))
        }
        _ => None,
    }
}

pub fn try_rewrite_kubectl(match_cmd: &str, cmd_body: &str) -> Option<String> {
    let after_kubectl = match_cmd.strip_prefix("kubectl ").unwrap_or("");
    let stripped = KUBECTL_GLOBAL_FLAGS_RE.replace_all(after_kubectl, "");
    let stripped = stripped.trim_start();
    let subcmd = stripped.split_whitespace().next().unwrap_or("");
    match subcmd {
        "get" | "logs" | "describe" | "apply" => {
            Some(replace_prefix(cmd_body, "kubectl ", "rtk kubectl "))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::rewrite_chain;

    fn rewrite(cmd: &str) -> Option<String> {
        if cmd.contains("<<") {
            return None;
        }
        rewrite_chain(cmd)
    }

    #[test]
    fn test_docker_ps() {
        assert_eq!(rewrite("docker ps"), Some("rtk docker ps".into()));
    }

    #[test]
    fn test_docker_compose() {
        assert_eq!(
            rewrite("docker compose up -d"),
            Some("rtk docker compose up -d".into())
        );
    }

    #[test]
    fn test_kubectl_get() {
        assert_eq!(
            rewrite("kubectl get pods"),
            Some("rtk kubectl get pods".into())
        );
    }
}
