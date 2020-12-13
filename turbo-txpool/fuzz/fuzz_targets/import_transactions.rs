#![no_main]

#[macro_use]
extern crate libfuzzer_sys;

use turbo_txpool::tx::Tx;
use turbo_txpool::{Config, TxPool};

fuzz_target!(|txs: Vec<Tx>| { target(txs) });

#[tokio::main]
async fn target(txs: Vec<Tx>) {
    let config = Config::builder().max_txs(1).build();

    let mut pool = TxPool::with_config(config);

    pool.insert_transactions(txs).await;
}
