use serde::{Deserialize, Serialize};

use snafu::{ResultExt, Snafu};

use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;

use structopt::StructOpt;

use turbo_txpool::config::Config;
use turbo_txpool::TxPool;

#[derive(Debug, Serialize, Deserialize)]
struct TonicConfig {
    bind: SocketAddr,
    #[serde(flatten)]
    config: Config,
}

#[derive(Debug, Snafu)]
enum Error {
    Tonic { source: tonic::transport::Error },
    Io { source: std::io::Error },
    Toml { source: toml::de::Error },
}

#[derive(Debug, StructOpt)]
struct Opt {
    #[structopt(short = "c", help = "Path to TOML configuration file")]
    config: PathBuf,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    let opt = Opt::from_args();
    let config_bytes = fs::read(&opt.config).context(Io)?;
    let tonic_config: TonicConfig =
        toml::de::from_slice(&config_bytes).context(Toml)?;

    TxPool::with_config(tonic_config.config)
        .await
        .context(Tonic)?
        .run(tonic_config.bind)
        .await
        .context(Tonic)?;

    Ok(())
}
