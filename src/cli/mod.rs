use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tonic::transport::Channel;

use crate::api::proto;
use proto::recalld_client::RecalldClient;
use proto::plugins_client::PluginsClient;

/// Connect to the running daemon's gRPC server.
async fn connect(addr: &str) -> Result<Channel> {
    let endpoint = format!("http://{addr}");
    Channel::from_shared(endpoint)?
        .connect()
        .await
        .context("failed to connect to recalld daemon — is it running?")
}

pub fn ocr_benchmark(manifest: &Path, variants: &[String], json: bool) -> Result<()> {
    let variants = crate::ocr::benchmark::resolve_variants(variants)?;
    let report = crate::ocr::benchmark::run_manifest(manifest, &variants)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", crate::ocr::benchmark::render_pretty(&report));
    }

    Ok(())
}

/// `recalld clean`
pub fn clean(
    config: &crate::config::Config,
    delete_dek: bool,
    restart_daemon: bool,
    restart_passphrase: Option<&str>,
) -> Result<()> {
    stop_daemon_from_pid_file(&config.pid_path())?;

    let mut removed: Vec<PathBuf> = Vec::new();

    remove_file_if_exists(&config.db_wal_path(), &mut removed)?;
    remove_file_if_exists(&config.db_shm_path(), &mut removed)?;
    remove_file_if_exists(&config.db_path(), &mut removed)?;
    remove_dir_if_exists(&config.screenshots_dir(), &mut removed)?;
    remove_file_if_exists(&config.pid_path(), &mut removed)?;

    if delete_dek {
        remove_file_if_exists(&config.key_path(), &mut removed)?;
    } else {
        tracing::info!(path = %config.key_path().display(), "preserving encryption key");
    }

    if removed.is_empty() {
        println!("No persisted runtime artifacts were present to clean.");
    } else {
        println!("Removed {} path(s):", removed.len());
        for path in &removed {
            println!("  {}", path.display());
        }
    }

    if restart_daemon {
        restart_daemon_process(restart_passphrase)?;
    }

    Ok(())
}

fn stop_daemon_from_pid_file(pid_path: &Path) -> Result<()> {
    if !pid_path.exists() {
        return Ok(());
    }

    let Some(pid) = read_pid(pid_path)? else {
        return Ok(());
    };

    if pid == std::process::id() as i32 {
        return Ok(());
    }

    if !is_process_alive(pid) {
        tracing::warn!(pid, path = %pid_path.display(), "found stale PID file");
        let _ = std::fs::remove_file(pid_path);
        return Ok(());
    }

    let process_name = process_name_for_pid(pid);
    if process_name.as_deref() != Some("recalld") {
        if !is_process_alive(pid) {
            tracing::warn!(pid, path = %pid_path.display(), "PID exited before cleanup; treating as stale");
            let _ = std::fs::remove_file(pid_path);
            return Ok(());
        }
        let found = process_name.unwrap_or_else(|| "<unknown>".to_string());
        bail!(
            "PID file {} points to pid {} ({found}), refusing to signal a non-recalld process",
            pid_path.display(),
            pid
        );
    }

    tracing::info!(pid, "stopping running daemon before cleanup");
    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            return Err(err).with_context(|| format!("failed to send SIGTERM to daemon pid {pid}"));
        }
    }

    let start = Instant::now();
    let timeout = Duration::from_secs(5);
    while start.elapsed() < timeout {
        if !is_process_alive(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if is_process_alive(pid) {
        bail!("daemon pid {pid} did not stop after SIGTERM; stop it manually and retry");
    }

    Ok(())
}

fn read_pid(pid_path: &Path) -> Result<Option<i32>> {
    let text = std::fs::read_to_string(pid_path)
        .with_context(|| format!("failed to read PID file {}", pid_path.display()))?;
    let raw = text.trim();
    if raw.is_empty() {
        return Ok(None);
    }

    match raw.parse::<i32>() {
        Ok(pid) if pid > 0 => Ok(Some(pid)),
        _ => {
            tracing::warn!(path = %pid_path.display(), value = %raw, "invalid PID file contents; deleting stale file");
            let _ = std::fs::remove_file(pid_path);
            Ok(None)
        }
    }
}

fn is_process_alive(pid: i32) -> bool {
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EPERM)
    )
}

fn process_name_for_pid(pid: i32) -> Option<String> {
    let cmdline_path = format!("/proc/{pid}/cmdline");
    let cmdline = std::fs::read(cmdline_path).ok()?;
    if cmdline.is_empty() {
        return None;
    }

    let end = cmdline
        .iter()
        .position(|b| *b == 0)
        .unwrap_or(cmdline.len());
    let argv0 = String::from_utf8_lossy(&cmdline[..end]);
    let name = Path::new(argv0.as_ref())
        .file_name()?
        .to_string_lossy()
        .to_string();
    Some(name)
}

fn remove_file_if_exists(path: &Path, removed: &mut Vec<PathBuf>) -> Result<()> {
    if path.exists() {
        std::fs::remove_file(path)
            .with_context(|| format!("failed to remove file {}", path.display()))?;
        removed.push(path.to_path_buf());
    }
    Ok(())
}

fn remove_dir_if_exists(path: &Path, removed: &mut Vec<PathBuf>) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("failed to remove directory {}", path.display()))?;
        removed.push(path.to_path_buf());
    }
    Ok(())
}

fn restart_daemon_process(passphrase: Option<&str>) -> Result<()> {
    let exe = std::env::current_exe().context("failed to resolve current executable")?;
    let mut cmd = Command::new(&exe);
    cmd.arg("daemon");
    if let Some(passphrase) = passphrase {
        cmd.arg("--passphrase").arg(passphrase);
    }
    let child = cmd
        .spawn()
        .with_context(|| format!("failed to restart daemon via {}", exe.display()))?;
    println!("Restarted daemon (pid {}).", child.id());
    Ok(())
}

/// `recalld status`
pub async fn status(addr: &str) -> Result<()> {
    let channel = connect(addr).await?;
    let mut client = RecalldClient::new(channel);

    let resp = client
        .status(proto::StatusRequest {})
        .await?
        .into_inner();

    println!("Status:          {}", if resp.running { "running" } else { "stopped" });
    println!("Uptime:          {}s", resp.uptime_seconds);
    println!("Total entries:   {}", resp.total_entries);
    println!("Last capture:    {}", if resp.last_capture_timestamp > 0 {
        format!("ts={}", resp.last_capture_timestamp)
    } else {
        "none".into()
    });
    println!("Capture backend: {}", resp.capture_backend);
    println!("Active plugins:  {}", resp.active_plugins);

    Ok(())
}

/// `recalld search <query>`
pub async fn search(addr: &str, query: &str, limit: u32) -> Result<()> {
    let channel = connect(addr).await?;
    let mut client = RecalldClient::new(channel);

    let resp = client
        .search(proto::SearchRequest {
            query: query.to_string(),
            limit,
            offset: 0,
        })
        .await?
        .into_inner();

    if resp.results.is_empty() {
        println!("No results found.");
        return Ok(());
    }

    for (i, result) in resp.results.iter().enumerate() {
        println!(
            "{}. [{:.2}] {} — {} (ts={})",
            i + 1,
            result.similarity,
            result.app,
            result.title,
            result.timestamp
        );
        // Print first 200 chars of OCR text
        let text_preview: String = result.text.chars().take(200).collect();
        if !text_preview.is_empty() {
            println!("   {text_preview}");
        }
        println!();
    }

    Ok(())
}

/// `recalld plugin list`
pub async fn plugin_list(addr: &str) -> Result<()> {
    let channel = connect(addr).await?;
    let mut client = PluginsClient::new(channel);

    let resp = client
        .list(proto::ListPluginsRequest {})
        .await?
        .into_inner();

    if resp.plugins.is_empty() {
        println!("No plugins found.");
        return Ok(());
    }

    for p in &resp.plugins {
        let status = if p.enabled { "enabled" } else { "disabled" };
        println!("  {} v{} [{}]", p.name, p.version, status);
        if !p.event_subscriptions.is_empty() {
            println!("    events: {}", p.event_subscriptions.join(", "));
        }
    }

    Ok(())
}

/// `recalld plugin enable <name>`
pub async fn plugin_enable(addr: &str, name: &str) -> Result<()> {
    let channel = connect(addr).await?;
    let mut client = PluginsClient::new(channel);

    let resp = client
        .enable(proto::PluginId {
            name: name.to_string(),
        })
        .await?
        .into_inner();

    if resp.success {
        println!("Plugin '{}' enabled.", name);
    } else {
        println!("Failed to enable '{}': {}", name, resp.message);
    }
    Ok(())
}

/// `recalld plugin disable <name>`
pub async fn plugin_disable(addr: &str, name: &str) -> Result<()> {
    let channel = connect(addr).await?;
    let mut client = PluginsClient::new(channel);

    let resp = client
        .disable(proto::PluginId {
            name: name.to_string(),
        })
        .await?
        .into_inner();

    if resp.success {
        println!("Plugin '{}' disabled.", name);
    } else {
        println!("Failed to disable '{}': {}", name, resp.message);
    }
    Ok(())
}
