//! Run the dependency's relay server with process-owned lifecycle control.

use anyhow::{Context, Result};
use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use std::io::{self, Write};
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<()> {
    let address: SocketAddr = std::env::args()
        .nth(1)
        .context("missing bind address")?
        .parse()?;
    let mut config = ServerConfig::default();
    config.relay = Some(RelayConfig::new(address));
    let server = Server::spawn(config).await?;
    writeln!(io::stdout().lock(), "ready")?;
    io::stdout().flush()?;
    tokio::signal::ctrl_c().await?;
    server.shutdown().await?;
    Ok(())
}
