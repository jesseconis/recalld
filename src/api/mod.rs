pub mod service;

use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tonic::transport::Server;

use crate::daemon::DaemonState;
use service::RecalldService;

pub mod proto {
    tonic::include_proto!("recalld");
}

/// Start the gRPC server.
pub async fn serve(
    state: Arc<DaemonState>,
    addr: SocketAddr,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let svc = RecalldService::new(state);

    let recalld_server = proto::recalld_server::RecalldServer::new(svc.clone());
    let plugins_server = proto::plugins_server::PluginsServer::new(svc);

    tracing::info!(%addr, "gRPC server listening");

    Server::builder()
        .add_service(recalld_server)
        .add_service(plugins_server)
        .serve_with_shutdown(addr, shutdown_signal(shutdown_rx))
        .await?;

    Ok(())
}

async fn shutdown_signal(rx: tokio::sync::watch::Receiver<bool>) {
    let mut rx = rx;
    while !*rx.borrow_and_update() {
        if rx.changed().await.is_err() {
            break;
        }
    }
}
