turbo-txpool
============

`turbo-txpool` is a modular transaction pool for Ethereum clients based on the [turbo-geth interfaces][tg].

[tg]: https://github.com/ledgerwatch/interfaces

## Configuration

The configuration file is written using the [TOML][toml] format, and has the following options:

```toml
bind = "127.0.0.1:54001"            # Expose the txpool service on this endpoint.
control = "http://127.0.0.1:9092"   # Location of the txpool_control service.
max_txs = 1024                      # Maximum number of transactions to store in the pool.
```

[toml]: https://toml.io/

## Usage

```
cargo run -- -c /path/to/configuration/file.toml
```
