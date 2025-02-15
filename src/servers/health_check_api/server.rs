//! Logic to run the Health Check HTTP API server.
//!
//! This API is intended to be used by the container infrastructure to check if
//! the whole application is healthy.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::routing::get;
use axum::{Json, Router};
use axum_server::Handle;
use futures::Future;
use log::info;
use serde_json::json;
use tokio::sync::oneshot::Sender;
use torrust_tracker_configuration::Configuration;

use crate::bootstrap::jobs::health_check_api::ApiServerJobStarted;
use crate::servers::health_check_api::handlers::health_check_handler;

/// Starts Health Check API server.
///
/// # Panics
///
/// Will panic if binding to the socket address fails.
pub fn start(
    socket_addr: SocketAddr,
    tx: Sender<ApiServerJobStarted>,
    config: Arc<Configuration>,
) -> impl Future<Output = Result<(), std::io::Error>> {
    let app = Router::new()
        .route("/", get(|| async { Json(json!({})) }))
        .route("/health_check", get(health_check_handler))
        .with_state(config);

    let handle = Handle::new();
    let cloned_handle = handle.clone();

    let tcp_listener = std::net::TcpListener::bind(socket_addr).expect("Could not bind tcp_listener to address.");
    let bound_addr = tcp_listener
        .local_addr()
        .expect("Could not get local_addr from tcp_listener.");

    tokio::task::spawn(async move {
        tokio::signal::ctrl_c().await.expect("Failed to listen to shutdown signal.");
        info!("Stopping Torrust Health Check API server o http://{} ...", bound_addr);
        cloned_handle.shutdown();
    });

    let running = axum_server::from_tcp(tcp_listener)
        .handle(handle)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>());

    tx.send(ApiServerJobStarted { bound_addr })
        .expect("the Health Check API server should not be dropped");

    running
}
