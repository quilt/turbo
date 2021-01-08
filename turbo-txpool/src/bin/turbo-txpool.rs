use std::net::Ipv6Addr;

use turbo_txpool::{Config, TxPool};

#[tokio::main]
async fn main() -> Result<(), tonic::transport::Error> {
    let config = Config::builder()
        .control(String::new()) // TODO
        .max_txs(1024)
        .build();

    TxPool::with_config(config)
        .await?
        .run((Ipv6Addr::LOCALHOST, 54001))
        .await
}
