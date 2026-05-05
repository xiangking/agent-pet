use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
enum FocusTarget {
    App(String),
    Url(String),
}

pub fn focus_target(target: &str) -> Result<(), String> {
    match parse_focus_target(target)? {
        FocusTarget::App(name) => focus_app(&name),
        FocusTarget::Url(url) => open_url(&url),
    }
}

fn parse_focus_target(target: &str) -> Result<FocusTarget, String> {
    let target = target.trim();
    if let Some(name) = target.strip_prefix("app:") {
        let name = name.trim();
        if name.is_empty() {
            return Err("Focus target app name is empty".to_string());
        }
        return Ok(FocusTarget::App(name.to_string()));
    }

    if let Some(url) = target.strip_prefix("url:") {
        let url = url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("Focus target URL must start with http:// or https://".to_string());
        }
        return Ok(FocusTarget::Url(url.to_string()));
    }

    Err("Focus target must start with app: or url:".to_string())
}

#[cfg(target_os = "macos")]
fn focus_app(name: &str) -> Result<(), String> {
    run_status(Command::new("open").arg("-a").arg(name), "activate app")
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> Result<(), String> {
    run_status(Command::new("open").arg(url), "open URL")
}

#[cfg(windows)]
fn focus_app(name: &str) -> Result<(), String> {
    let escaped = name.replace('\'', "''");
    let script = format!(
        "$ws = New-Object -ComObject WScript.Shell; if ($ws.AppActivate('{}')) {{ exit 0 }} else {{ exit 1 }}",
        escaped
    );
    run_status(
        Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(script),
        "activate app",
    )
}

#[cfg(windows)]
fn open_url(url: &str) -> Result<(), String> {
    run_status(
        Command::new("cmd").arg("/C").arg("start").arg("").arg(url),
        "open URL",
    )
}

#[cfg(all(not(target_os = "macos"), not(windows)))]
fn focus_app(_name: &str) -> Result<(), String> {
    Err("App focusing is not supported on this platform yet".to_string())
}

#[cfg(all(not(target_os = "macos"), not(windows)))]
fn open_url(url: &str) -> Result<(), String> {
    run_status(Command::new("xdg-open").arg(url), "open URL")
}

fn run_status(command: &mut Command, action: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|e| format!("Failed to {}: {}", action, e))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("Failed to {}: exit status {}", action, status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_focus_target_accepts_app() {
        assert_eq!(
            parse_focus_target("app:Codex").unwrap(),
            FocusTarget::App("Codex".to_string())
        );
    }

    #[test]
    fn parse_focus_target_accepts_url() {
        assert_eq!(
            parse_focus_target("url:http://127.0.0.1:18789/").unwrap(),
            FocusTarget::Url("http://127.0.0.1:18789/".to_string())
        );
    }

    #[test]
    fn parse_focus_target_rejects_unknown_scheme() {
        assert!(parse_focus_target("file:/tmp/x").is_err());
    }

    #[test]
    fn parse_focus_target_rejects_empty_app() {
        assert!(parse_focus_target("app: ").is_err());
    }
}
