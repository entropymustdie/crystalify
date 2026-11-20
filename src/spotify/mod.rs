pub mod auth;
pub mod client;
pub mod logger;
pub mod models;
pub mod player;

pub fn init_tls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
