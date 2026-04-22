//! Simplest possible Vera client — calls the `echo` connector and
//! prints the response. Demonstrates the `vera-client` SDK.
//!
//! Usage:
//! ```bash
//! export VERA_URL="https://18.190.188.99:8443"
//! export VERA_TOKEN="your_bearer_token_here"
//! export VERA_INSECURE=1
//! cargo run -p demo-echo -- "Hello, Vera!"
//! ```

use vera_client::VeraClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("VERA_URL").unwrap_or_else(|_| "https://localhost:8443".into());
    let token = std::env::var("VERA_TOKEN").expect("VERA_TOKEN env var required");

    let message = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "hello world".into());

    let accept_invalid_certs = std::env::var("VERA_INSECURE").is_ok();
    let client = VeraClient::builder()
        .url(&url)
        .bearer(&token)
        .danger_accept_invalid_certs(accept_invalid_certs)
        .build()?;

    println!("→ POST /v1/infer/echo: {message}");
    match client.infer("echo", message.as_bytes()).await {
        Ok(resp) => {
            println!(
                "← {} ({}): {}",
                resp.status,
                resp.body.len(),
                String::from_utf8_lossy(&resp.body)
            );
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}
