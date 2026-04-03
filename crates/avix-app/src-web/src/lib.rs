mod routes;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{routing::post, Router};
use clap::Parser;
use tokio::sync::broadcast;
use tower_http::{
    cors::CorsLayer,
    services::{ServeDir, ServeFile},
};
use tracing::info;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt};

use avix_client_core::{config::ClientConfig, persistence, state::new_shared};

use routes::{events_handler, invoke_handler, WebState};

#[derive(Parser, Debug)]
#[command(name = "avix-web", about = "Avix web UI server")]
struct Args {
    /// TCP port for the web UI to listen on.
    #[arg(long, default_value = "8080", env = "AVIX_WEB_PORT")]
    port: u16,

    /// Embedded mode: start the avix daemon on this ATP port and connect to it.
    /// Ignored when --server-url is set.
    #[arg(long, default_value = "9142", env = "AVIX_ATP_PORT")]
    atp_port: u16,

    /// External mode: connect to an already-running avix server at this URL.
    /// When set, no embedded daemon is started.
    #[arg(long, env = "AVIX_SERVER_URL")]
    server_url: Option<String>,

    /// Path to the Avix runtime root (embedded mode only).
    #[arg(long, env = "AVIX_ROOT")]
    root: Option<PathBuf>,

    /// Path to the frontend dist/ directory.
    #[arg(long, env = "AVIX_WEB_DIST", default_value = "dist")]
    dist: PathBuf,

    /// Log verbosity level (error, warn, info, debug, trace).
    #[arg(long = "log", default_value_t = LevelFilter::INFO, env = "AVIX_LOG")]
    log: LevelFilter,

    /// Enable structured trace output (comma-separated: atp,agent,notifications or all).
    #[arg(long, env = "AVIX_TRACE")]
    trace: Option<String>,
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let log_dir = persistence::app_data_dir().join("logs");
    let appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "avix-web");
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(appender)
                .with_target(false)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true)
                .json(),
        )
        .with(args.log)
        .init();

    info!(
        log_dir = %log_dir.display(),
        level = ?args.log,
        "avix-web starting on port {}",
        args.port
    );

    let mut config = ClientConfig::load().unwrap_or_else(|_| ClientConfig::default());

    if let Some(url) = &args.server_url {
        // External mode — connect to a pre-running avix server; no daemon spawn.
        config.server_url = url.clone();
        config.auto_start_server = false;
        info!("External mode: connecting to ATP server at {url}");
    } else {
        // Embedded mode — start the daemon on atp_port, derive server_url from it.
        config.server_url = format!("http://localhost:{}", args.atp_port);
        config.auto_start_server = false; // we manage the process below
        let root = args
            .root
            .clone()
            .unwrap_or_else(|| persistence::app_data_dir().join("runtime"));
        let trace_flags = args
            .trace
            .as_deref()
            .map(avix_core::trace::TraceFlags::from_csv)
            .unwrap_or_default();
        let runtime = avix_core::bootstrap::Runtime::bootstrap_with_root(&root)
            .await?
            .with_trace_flags(trace_flags);
        let atp_port = args.atp_port;
        tokio::spawn(async move {
            if let Err(e) = runtime.start_daemon(atp_port, false).await {
                tracing::error!("Embedded daemon failed: {e}");
            }
        });
        info!("Embedded mode: daemon on :{atp_port}");

        // Wait for the daemon's TCP port to be ready before attempting auto-login.
        // Without this, init() races the daemon and the connection always fails.
        let addr = format!("127.0.0.1:{atp_port}");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if tokio::net::TcpStream::connect(&addr).await.is_ok() {
                info!("Embedded daemon TCP ready on :{atp_port}");
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(
                    "Embedded daemon did not start within 15s — proceeding without auto-login"
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    let app_state = new_shared(config);
    {
        let mut guard = app_state.write().await;
        if let Err(e) = guard.init().await {
            tracing::warn!("State init error (will show login): {e}");
        }
    }

    // Broadcast channel for forwarding daemon events to WebSocket clients.
    let (events_tx, _) = broadcast::channel::<String>(128);
    let tx_clone = events_tx.clone();
    let emit_callback = Arc::new(move |event: &str, data: &serde_json::Value| {
        let msg = serde_json::json!({"event": event, "data": data}).to_string();
        let _ = tx_clone.send(msg);
    });
    app_state.write().await.set_emit_callback(emit_callback);

    let web_state = WebState {
        app: app_state,
        events_tx,
    };

    // Serve frontend static files with SPA fallback to index.html.
    let index_path = args.dist.join("index.html");
    let serve_dir = ServeDir::new(&args.dist).not_found_service(ServeFile::new(&index_path));

    let app = Router::new()
        .route("/api/invoke", post(invoke_handler))
        .route("/api/events", axum::routing::get(events_handler))
        .with_state(web_state)
        .fallback_service(serve_dir)
        .layer(CorsLayer::permissive());

    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    info!("Listening on http://{addr}");
    info!("Serving frontend from {}", args.dist.display());

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
