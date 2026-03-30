use anyhow::{Context, Result};
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
