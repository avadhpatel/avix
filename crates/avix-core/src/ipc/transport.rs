/// Creates an in-memory socket pair for testing.
/// Returns (client_stream, server_stream).
#[cfg(unix)]
pub async fn test_socket_pair() -> (tokio::net::UnixStream, tokio::net::UnixStream) {
    tokio::net::UnixStream::pair().unwrap()
}
