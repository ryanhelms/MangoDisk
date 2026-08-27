mod arguments;
mod core_runner;
mod errors;
mod execution_tokens;
mod http_server;
mod redaction;
mod server;
mod tools;

use std::sync::Arc;

use clap::Parser;
use mangodisk_core::{configure_application_paths, ApplicationPaths};
use mangodisk_platform::application_directories;
use rmcp::{transport::stdio, ServiceExt};

use crate::{
    arguments::Cli,
    server::{AdapterState, MangoDiskServer},
};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Stdout carries JSON-RPC in stdio mode, so logging goes to stderr only.
    env_logger::Builder::from_env(env_logger::Env::new().filter_or("MANGODISK_LOG", "error"))
        .target(env_logger::Target::Stderr)
        .format_timestamp_millis()
        .init();

    let cli = Cli::parse();
    if let Err(error) = configure_storage() {
        eprintln!("failed to configure MangoDisk storage: {error}");
        return std::process::ExitCode::FAILURE;
    }

    let state = Arc::new(AdapterState::new(
        cli.include_full_paths,
        cli.mutations_enabled(),
    ));
    log::info!(
        "mcp_server_starting transport={} mutations_enabled={} include_full_paths={}",
        if cli.http { "http" } else { "stdio" },
        state.mutations_enabled,
        state.include_full_paths
    );

    let result = if cli.http {
        http_server::serve_http(state, cli.bind, cli.port, cli.allowed_hosts).await
    } else {
        serve_stdio(state).await
    };
    if let Err(error) = result {
        eprintln!("mangodisk-mcp failed: {error}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

fn configure_storage() -> Result<(), String> {
    let directories = application_directories(mangodisk_core::APPLICATION_IDENTIFIER)
        .map_err(|error| error.to_string())?;
    let paths = ApplicationPaths::from_base_directories(
        directories.local_data_directory,
        directories.cache_directory,
    )
    .map_err(|error| error.to_string())?;
    configure_application_paths(paths).map_err(|error| error.to_string())
}

/// Serves MCP over stdin/stdout until the client closes stdin (EOF) or the
/// session ends. The serving future resolves after the initialize handshake,
/// so `waiting` only returns on transport shutdown.
async fn serve_stdio(state: Arc<AdapterState>) -> Result<(), String> {
    let server = MangoDiskServer::new(state);
    let service = server
        .serve(stdio())
        .await
        .map_err(|error| format!("the MCP initialize handshake failed: {error}"))?;
    let reason = service
        .waiting()
        .await
        .map_err(|error| format!("the MCP server task failed: {error}"))?;
    log::info!("mcp_server_stopped reason={reason:?}");
    Ok(())
}
