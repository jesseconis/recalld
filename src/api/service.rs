use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::daemon::DaemonState;

use super::proto;

#[derive(Clone)]
pub struct RecalldService {
    state: Arc<DaemonState>,
}

impl RecalldService {
    pub fn new(state: Arc<DaemonState>) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl proto::recalld_server::Recalld for RecalldService {
    async fn search(
        &self,
        request: Request<proto::SearchRequest>,
    ) -> Result<Response<proto::SearchResponse>, Status> {
        let req = request.into_inner();
        let limit = if req.limit == 0 {
            self.state.config.search_limit() as usize
        } else {
            req.limit as usize
        };

        let query_embedding = crate::embedding::embed(&req.query)
            .map_err(|e| Status::internal(format!("embedding failed: {e}")))?;

        let results = self
            .state
            .storage
            .search(&query_embedding, limit)
            .map_err(|e| Status::internal(format!("search failed: {e}")))?;

        let results = results
            .into_iter()
            .map(|r| proto::SearchResult {
                id: r.id,
                app: r.app,
                title: r.title,
                text: r.text,
                timestamp: r.timestamp,
                similarity: r.similarity,
                screenshot_filename: r.screenshot_filename,
            })
            .collect();

        Ok(Response::new(proto::SearchResponse { results }))
    }

    async fn timeline(
        &self,
        request: Request<proto::TimelineRequest>,
    ) -> Result<Response<proto::TimelineResponse>, Status> {
        let req = request.into_inner();
        let from = if req.from_timestamp == 0 { 0 } else { req.from_timestamp };
        let to = if req.to_timestamp == 0 { i64::MAX } else { req.to_timestamp };
        let limit = if req.limit == 0 { 100 } else { req.limit };

        let entries = self
            .state
            .storage
            .timeline(from, to, limit)
            .map_err(|e| Status::internal(format!("timeline query failed: {e}")))?;

        let entries = entries
            .into_iter()
            .map(|e| proto::TimelineEntry {
                id: e.id,
                app: e.app,
                title: e.title,
                timestamp: e.timestamp,
                screenshot_filename: e.screenshot_filename,
            })
            .collect();

        Ok(Response::new(proto::TimelineResponse { entries }))
    }

    async fn get_screenshot(
        &self,
        request: Request<proto::ScreenshotRequest>,
    ) -> Result<Response<proto::ScreenshotResponse>, Status> {
        let filename = request.into_inner().filename;
        let data = self
            .state
            .storage
            .get_screenshot(&filename)
            .map_err(|e| Status::not_found(format!("screenshot not found: {e}")))?;

        Ok(Response::new(proto::ScreenshotResponse {
            image_data: data,
            content_type: "image/webp".into(),
        }))
    }

    async fn status(
        &self,
        _request: Request<proto::StatusRequest>,
    ) -> Result<Response<proto::StatusResponse>, Status> {
        let uptime = self.state.start_time.elapsed().as_secs() as i64;
        let total = self
            .state
            .storage
            .count()
            .map_err(|e| Status::internal(e.to_string()))?;
        let last_ts = self
            .state
            .storage
            .latest_timestamp()
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(proto::StatusResponse {
            running: true,
            uptime_seconds: uptime,
            total_entries: total,
            last_capture_timestamp: last_ts,
            capture_backend: self.state.config.capture.backend.clone(),
            active_plugins: 0, // TODO: wire up plugin manager
        }))
    }

    async fn get_config(
        &self,
        _request: Request<proto::GetConfigRequest>,
    ) -> Result<Response<proto::GetConfigResponse>, Status> {
        let toml_str = toml::to_string_pretty(&self.state.config)
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(proto::GetConfigResponse {
            config_toml: toml_str,
        }))
    }

    async fn set_config(
        &self,
        _request: Request<proto::SetConfigRequest>,
    ) -> Result<Response<proto::SetConfigResponse>, Status> {
        // Config updates at runtime are complex (need to restart scheduler, etc.).
        // For now, suggest editing the file and restarting.
        Err(Status::unimplemented(
            "runtime config update not yet supported — edit config.toml and restart the daemon",
        ))
    }
}

#[tonic::async_trait]
impl proto::plugins_server::Plugins for RecalldService {
    async fn list(
        &self,
        _request: Request<proto::ListPluginsRequest>,
    ) -> Result<Response<proto::ListPluginsResponse>, Status> {
        // TODO: wire up plugin manager reference
        Ok(Response::new(proto::ListPluginsResponse {
            plugins: vec![],
        }))
    }

    async fn enable(
        &self,
        _request: Request<proto::PluginId>,
    ) -> Result<Response<proto::PluginActionResponse>, Status> {
        Err(Status::unimplemented("plugin enable not yet wired up"))
    }

    async fn disable(
        &self,
        _request: Request<proto::PluginId>,
    ) -> Result<Response<proto::PluginActionResponse>, Status> {
        Err(Status::unimplemented("plugin disable not yet wired up"))
    }
}
