use std::net::Ipv4Addr;

use turbo_txpool::config::Config;
use turbo_txpool::TxPool;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), tonic::transport::Error> {
    let config = Config::builder()
        .control("http://127.0.0.1:9092") // TODO
        .max_txs(1024)
        .build();

    TxPool::with_config(config)
        .await?
        .run((Ipv4Addr::LOCALHOST, 54001))
        .await
}
