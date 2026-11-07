#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod auth;
mod http;
mod models;
mod provider;

fn main() {
    agent2api_provider_sdk::serve_blocking(vec![Box::new(provider::FreePiProvider)]);
}
