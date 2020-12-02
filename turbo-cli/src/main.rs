use hex::FromHex;

use std::fmt;

use structopt::StructOpt;

use tonic::transport::channel::Channel;
use tonic::transport::Uri;

use turbo_proto::txpool::txpool_client::TxpoolClient;
use turbo_proto::txpool::txpool_control_client::TxpoolControlClient;
use turbo_proto::txpool::{
    AccountInfoRequest, GetTransactionsRequest, ImportRequest, TxHashes,
};

mod cmd {
    use super::*;

    #[derive(Debug)]
    enum HexError<E> {
        NotHex,
        FromHex(E),
    }

    impl<E> fmt::Display for HexError<E>
    where
        E: fmt::Display,
    {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            match self {
                HexError::NotHex => write!(f, "argument requires `0x` prefix"),
                HexError::FromHex(e) => e.fmt(f),
            }
        }
    }

    fn hex<T>(txt: &str) -> Result<T, HexError<T::Error>>
    where
        T: FromHex,
    {
        if let Some(stripped) = txt.strip_prefix("0x") {
            T::from_hex(stripped).map_err(HexError::FromHex)
        } else {
            Err(HexError::NotHex)
        }
    }

    #[derive(Debug, StructOpt)]
    pub struct TxAccountInfo {
        #[structopt(parse(try_from_str=hex))]
        block_hash: [u8; 32],

        #[structopt(parse(try_from_str=hex))]
        account: [u8; 20],
    }

    impl TxAccountInfo {
        pub async fn run(
            self,
            mut client: TxpoolControlClient<Channel>,
        ) -> Result<(), Box<dyn std::error::Error>> {
            let resp = client
                .account_info(AccountInfoRequest {
                    block_hash: Vec::from(&self.block_hash as &[u8]),
                    account: Vec::from(&self.account as &[u8]),
                })
                .await?;

            println!("{:#?}", resp);

            Ok(())
        }
    }

    #[derive(Debug, StructOpt)]
    pub enum TxControl {
        AccountInfo(TxAccountInfo),
    }

    impl TxControl {
        pub async fn run(
            self,
            dst: Uri,
        ) -> Result<(), Box<dyn std::error::Error>> {
            let client = TxpoolControlClient::connect(dst).await?;

            match self {
                TxControl::AccountInfo(acct) => acct.run(client).await,
            }
        }
    }

    #[derive(Debug, StructOpt)]
    pub struct TxUnknown {
        #[structopt(parse(try_from_str=hex))]
        hashes: Vec<[u8; 32]>,
    }

    impl TxUnknown {
        pub async fn run(
            self,
            dst: Uri,
        ) -> Result<(), Box<dyn std::error::Error>> {
            let hashes: Vec<_> =
                self.hashes.into_iter().map(Vec::from).collect();

            let mut client = TxpoolClient::connect(dst).await?;
            let txs = client
                .find_unknown_transactions(TxHashes { hashes })
                .await?;

            println!("{:?}", txs);

            Ok(())
        }
    }

    #[derive(Debug, StructOpt)]
    pub struct TxGet {
        #[structopt(parse(try_from_str=hex))]
        hashes: Vec<[u8; 32]>,
    }

    impl TxGet {
        pub async fn run(
            self,
            dst: Uri,
        ) -> Result<(), Box<dyn std::error::Error>> {
            let hashes: Vec<_> =
                self.hashes.into_iter().map(Vec::from).collect();

            let mut client = TxpoolClient::connect(dst).await?;
            let txs = client
                .get_transactions(GetTransactionsRequest { hashes })
                .await?;

            println!("{:?}", txs);

            Ok(())
        }
    }

    #[derive(Debug, StructOpt)]
    pub struct TxImport {
        #[structopt(parse(try_from_str=hex))]
        txs: Vec<Vec<u8>>,
    }

    impl TxImport {
        pub async fn run(
            self,
            dst: Uri,
        ) -> Result<(), Box<dyn std::error::Error>> {
            let mut client = TxpoolClient::connect(dst).await?;
            let txs = client
                .import_transactions(ImportRequest { txs: self.txs })
                .await?;

            println!("{:?}", txs);

            Ok(())
        }
    }

    #[derive(Debug, StructOpt)]
    pub enum TxPool {
        Unknown(TxUnknown),
        Import(TxImport),
        Get(TxGet),
        Control(TxControl),
    }

    impl TxPool {
        pub async fn run(
            self,
            dst: Uri,
        ) -> Result<(), Box<dyn std::error::Error>> {
            match self {
                TxPool::Unknown(un) => un.run(dst).await,
                TxPool::Import(import) => import.run(dst).await,
                TxPool::Get(get) => get.run(dst).await,
                TxPool::Control(ctrl) => ctrl.run(dst).await,
            }
        }
    }

    #[derive(Debug, StructOpt)]
    pub enum Command {
        TxPool(TxPool),
    }

    impl Command {
        pub async fn run(
            self,
            dst: Uri,
        ) -> Result<(), Box<dyn std::error::Error>> {
            match self {
                cmd::Command::TxPool(tx) => tx.run(dst).await,
            }
        }
    }
}

#[derive(Debug, StructOpt)]
#[structopt(setting=structopt::clap::AppSettings::VersionlessSubcommands)]
struct Opt {
    #[structopt(short = "d", long = "destination")]
    destination: Uri,
    #[structopt(subcommand)]
    command: cmd::Command,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opts = Opt::from_args();

    opts.command.run(opts.destination).await
}
