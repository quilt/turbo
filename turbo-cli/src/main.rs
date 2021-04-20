// Copyright 2021 ConsenSys
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Testing harness for interacting with the `txpool` and `txpool_control` gRPC
//! endpoints.

#![deny(unsafe_code, missing_docs, missing_debug_implementations)]

use hex::FromHex;

use std::fmt;

use structopt::StructOpt;

use tonic::transport::channel::Channel;
use tonic::transport::Uri;

use ethereum_interfaces::txpool::txpool_client::TxpoolClient;
use ethereum_interfaces::txpool::txpool_control_client::TxpoolControlClient;
use ethereum_interfaces::txpool::{
    AccountInfoRequest, GetTransactionsRequest, ImportRequest, TxHashes,
};

use ethereum_types::{Address, H256};

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
                    block_hash: Some(H256::from(&self.block_hash).into()),
                    account: Some(Address::from(&self.account).into()),
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
            let hashes: Vec<_> = self
                .hashes
                .into_iter()
                .map(H256::from)
                .map(Into::into)
                .collect();

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
            let hashes: Vec<_> = self
                .hashes
                .into_iter()
                .map(H256::from)
                .map(Into::into)
                .collect();

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
            let txs: Vec<_> = self.txs.into_iter().map(Into::into).collect();
            let mut client = TxpoolClient::connect(dst).await?;
            let txs = client.import_transactions(ImportRequest { txs }).await?;

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

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opts = Opt::from_args();

    opts.command.run(opts.destination).await
}
