use tracing::instrument;

#[tokio::main]
#[instrument]
async fn main() {
    if let Err(e) = avix_web::run().await {
        eprintln!("avix-web error: {e}");
        std::process::exit(1);
    }
}
