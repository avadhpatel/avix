use avix_client_core::config::ClientConfig;
use avix_client_core::state::new_shared;

pub fn create_app_state() -> avix_client_core::state::SharedState {
    let config = ClientConfig::default();
    new_shared(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_app_state_with_default_config() {
        let state = create_app_state();
        let s = state.try_read().unwrap();
        assert_eq!(s.config.server_url, \"http://127.0.0.1:7700\");
        assert_eq!(s.connection_status, avix_client_core::state::ConnectionStatus::Disconnected);
    }
}