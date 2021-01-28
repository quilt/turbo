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

//! An implementation of turbo-geth's transaction pool interfaces for the
//! Ethereum network.

#![deny(unsafe_code, missing_docs, missing_debug_implementations)]

pub mod config;
mod control;
pub mod error;
pub mod tx;

#[cfg(feature = "arbitrary")]
extern crate arbitrary_dep as arbitrary;

use crate::config::Config;
use crate::control::{Control, PbControl};
use crate::error::{import_error, ImportError};
use crate::tx::{Tx, VerifiedTx};

use ethereum_types::{Address, H256, U256};

use slab::Slab;

use snafu::{ensure, OptionExt, ResultExt};

use std::collections::{hash_map, BTreeSet, HashMap};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use tokio_stream::StreamExt;

use tonic::transport::Server;
use tonic::{Request, Response, Status};

use turbo_proto::txpool::block_stream_request::StartWith;
use turbo_proto::txpool::txpool_control_client as client;
use turbo_proto::txpool::txpool_server as server;
use turbo_proto::txpool::{
    account_diff, block_diff, AccountDiff, AccountInfoRequest, AppliedBlock,
    BlockDiff, BlockStreamRequest, GetTransactionsReply,
    GetTransactionsRequest, ImportReply, ImportRequest, ImportResult,
    RevertedBlock, TxHashes,
};

#[derive(Debug, Ord, PartialOrd, Eq, PartialEq)]
struct Priced {
    pub gas_price: U256,
    pub key: usize,
}

#[derive(Debug)]
struct Inner<C> {
    control: C,
    txs: Slab<VerifiedTx>,
    by_hash: HashMap<H256, usize>,
    by_price: BTreeSet<Priced>,

    latest_block: Option<H256>,
    max_txs: usize,
}

impl<C> Inner<C> {
    fn cheapest_key(&self) -> Option<usize> {
        self.by_price.iter().next().map(|p| p.key)
    }

    fn cheapest(&self) -> Option<&VerifiedTx> {
        self.cheapest_key().map(|k| &self.txs[k])
    }

    fn by_hash(&self, hash: &H256) -> Option<&VerifiedTx> {
        self.by_hash.get(hash).and_then(|k| self.txs.get(*k))
    }

    fn remove(&mut self, key: usize) {
        let tx = self.txs.remove(key);
        self.by_hash.remove(tx.hash()).expect("desync by hash");

        let removed = self.by_price.remove(&Priced {
            gas_price: *tx.gas_price(),
            key,
        });
        assert!(removed, "desync by price");
    }

    pub fn find_unknown_transactions(
        &self,
        request: Request<TxHashes>,
    ) -> Result<Response<TxHashes>, Status> {
        let hashes = request
            .into_inner()
            .hashes
            .into_iter()
            .filter(|vec| !self.by_hash.contains_key(&H256::from_slice(&vec)))
            .collect();

        Ok(Response::new(TxHashes { hashes }))
    }

    pub fn with_config(control: C, config: Config) -> Self {
        Self {
            latest_block: None,
            max_txs: config.max_txs(),
            txs: Slab::with_capacity(config.max_txs()),
            by_hash: HashMap::with_capacity(config.max_txs()),
            by_price: BTreeSet::new(),
            control,
        }
    }

    pub fn get_transactions(
        &self,
        request: Request<GetTransactionsRequest>,
    ) -> Result<Response<GetTransactionsReply>, Status> {
        let txs: Vec<_> = request
            .into_inner()
            .hashes
            .into_iter()
            .filter_map(|vec| self.by_hash(&H256::from_slice(&vec)))
            .map(|verified| {
                let mut stream = rlp::RlpStream::new();
                verified.tx().encode(&mut stream);
                stream.as_raw().to_vec()
            })
            .collect();

        Ok(Response::new(GetTransactionsReply { txs }))
    }

    fn insert(&mut self, verified: VerifiedTx) -> Result<(), ImportError> {
        let by_hash = match self.by_hash.entry(*verified.hash()) {
            hash_map::Entry::Vacant(v) => v,
            hash_map::Entry::Occupied(_) => {
                return Err(ImportError::AlreadyExists {
                    txhash: *verified.hash(),
                })
            }
        };

        let gas_price = *verified.gas_price();
        let key = self.txs.insert(verified);

        let inserted = self.by_price.insert(Priced { gas_price, key });
        assert!(inserted);

        by_hash.insert(key);

        while self.txs.len() > self.max_txs {
            self.remove(self.cheapest_key().unwrap());
        }

        Ok(())
    }
}

impl<C> Inner<C>
where
    C: Clone + Control,
{
    fn recheck(&mut self, diffs: &[AccountDiff]) {
        #[derive(Default)]
        struct Account {
            balance: U256,
            nonce: U256,
        }

        let mut accounts = HashMap::with_capacity(diffs.len());
        for diff in diffs {
            let account;
            let address;

            match &diff.diff {
                Some(account_diff::Diff::Deleted(addr)) => {
                    account = Account::default();
                    address = Address::from_slice(&addr);
                }
                Some(account_diff::Diff::Changed(delta)) => {
                    address = Address::from_slice(&delta.address);
                    account = Account {
                        balance: U256::from_big_endian(&delta.balance),
                        nonce: U256::from_big_endian(&delta.nonce),
                    };
                }
                None => continue,
            };

            accounts.insert(address, account);
        }

        let mut to_remove = Vec::new();
        for (key, tx) in self.txs.iter() {
            let account = match accounts.get(tx.from()) {
                Some(a) => a,
                None => continue,
            };

            if Self::validate(tx, account.nonce, account.balance).is_err() {
                to_remove.push(key);
            }
        }

        for key in to_remove.into_iter() {
            self.remove(key);
        }
    }

    async fn block_reverted(&mut self, block: RevertedBlock) {
        self.latest_block = Some(H256::from_slice(&block.new_hash));

        let req = ImportRequest {
            txs: block.reverted_transactions,
        };

        self.recheck(&block.new_state);
        self.import_transactions_request(req).await.ok();
    }

    async fn block_applied(&mut self, block: AppliedBlock) {
        let hash = H256::from_slice(&block.hash);
        self.latest_block = Some(hash);
        self.recheck(&block.account_diffs);
    }

    async fn block_diff(&mut self, block_diff: BlockDiff) {
        let diff = match block_diff.diff {
            Some(d) => d,
            None => return,
        };

        match diff {
            block_diff::Diff::Applied(a) => self.block_applied(a).await,
            block_diff::Diff::Reverted(r) => self.block_reverted(r).await,
        }
    }

    async fn qualify(&self, tx: Tx) -> Result<VerifiedTx, ImportError> {
        let latest_block = self.latest_block.context(import_error::NotReady)?;

        if self.txs.len() >= self.max_txs {
            let cheapest =
                self.cheapest().map(|t| *t.gas_price()).unwrap_or_default();

            ensure!(
                tx.gas_price > cheapest,
                import_error::FeeTooLow { minimum: cheapest },
            );
        }

        let verified = VerifiedTx::new(tx)
            .map_err(|e| Box::new(e).into())
            .context(import_error::Ecdsa)?;

        let req = AccountInfoRequest {
            account: verified.from().as_bytes().to_owned(),
            block_hash: Vec::from(latest_block.to_fixed_bytes()),
        };

        let mut control = self.control.clone();
        let account = control
            .account_info(req)
            .await
            .context(import_error::RequestFailed)?;

        let nonce = U256::from_big_endian(&account.nonce);
        let balance = U256::from_big_endian(&account.balance);

        Self::validate(&verified, nonce, balance)?;
        Ok(verified)
    }

    fn validate(
        verified: &VerifiedTx,
        nonce: U256,
        balance: U256,
    ) -> Result<(), ImportError> {
        ensure!(
            U256::from(verified.nonce()) == nonce,
            import_error::InvalidNonce {
                txhash: *verified.hash(),
            }
        );

        let required =
            (verified.gas_price() * verified.gas_limit()) + verified.value();

        ensure!(
            balance >= required,
            import_error::InsufficientBalance {
                txhash: *verified.hash(),
            }
        );

        Ok(())
    }

    pub async fn insert_transactions(
        &mut self,
        txs: Vec<Tx>,
    ) -> Vec<ImportResult> {
        let verified: Vec<_> = futures_util::future::join_all(
            txs.into_iter().map(|tx| self.qualify(tx)),
        )
        .await;

        let mut result = Vec::with_capacity(verified.len());
        for res in verified.into_iter() {
            let ins = match res.and_then(|vx| self.insert(vx)) {
                Ok(()) => ImportResult::Success,
                Err(e) => e.into(),
            };
            result.push(ins);
        }

        result
    }

    pub async fn import_transactions(
        &mut self,
        request: Request<ImportRequest>,
    ) -> Result<Response<ImportReply>, Status> {
        self.import_transactions_request(request.into_inner())
            .await
            .map(Response::new)
    }

    async fn import_transactions_request(
        &mut self,
        request: ImportRequest,
    ) -> Result<ImportReply, Status> {
        let qualifiers = request.txs.into_iter().map(|b| {
            let decoded = Tx::decode(&rlp::Rlp::new(&b));

            async { self.qualify(decoded?).await }
        });

        let txs = futures_util::future::join_all(qualifiers).await;

        let mut imported = Vec::with_capacity(txs.len());

        for tx in txs.into_iter() {
            let result = match tx.and_then(|tx| self.insert(tx)) {
                Ok(()) => ImportResult::Success,
                Err(e) => e.into(),
            };

            imported.push(result as i32);
        }

        Ok(ImportReply { imported })
    }
}

/// An implementation of turbo-geth's transaction pool interfaces for the
/// Ethereum network.
///
/// # Example
///
/// ```no_run
/// use turbo_txpool::TxPool;
/// use turbo_txpool::config::Config;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let config = Config::builder()
///     .max_txs(1024)
///     .control("http://127.0.0.1:9092")
///     .build();
///
/// let pool = TxPool::with_config(config).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct TxPool {
    inner: Arc<RwLock<Inner<PbControl>>>,
    background: Option<JoinHandle<()>>,
}

impl TxPool {
    /// Construct a new `TxPool` instance with the given configuration.
    pub async fn with_config(
        config: Config,
    ) -> Result<Self, tonic::transport::Error> {
        let client =
            client::TxpoolControlClient::connect(config.control().to_owned())
                .await?;
        let control = PbControl::new(client);
        let mut stream_control = control.clone();

        let inner = Arc::new(RwLock::new(Inner::with_config(control, config)));
        let stream_inner = inner.clone();

        let background = tokio::spawn(async move {
            let request = BlockStreamRequest {
                start_with: Some(StartWith::Latest(())),
            };

            let mut stream =
                stream_control.block_stream(request).await.unwrap();

            while let Some(resp) = stream.next().await {
                if let Ok(diff) = resp {
                    let mut guard = stream_inner.write().await;
                    guard.block_diff(diff).await;
                }
            }
        });

        Ok(Self {
            inner,
            background: Some(background),
        })
    }

    /// Begin servicing requests, binding to `addr`.
    pub async fn run<I>(
        mut self,
        addr: I,
    ) -> Result<(), tonic::transport::Error>
    where
        I: Into<SocketAddr>,
    {
        let background = self.background.take().unwrap();

        let server = Server::builder()
            .add_service(server::TxpoolServer::new(self))
            .serve(addr.into());

        tokio::select! {
            server_result = server => server_result,
            bg_result = background => {
                bg_result.expect("background task exited with panic");
                panic!("background task exited unexpectedly (without panic)");
            }
        }
    }

    /// Given a list of transaction hashes, return the ones unknown to this
    /// transaction pool.
    pub async fn find_unknown_transactions(
        &self,
        request: Request<TxHashes>,
    ) -> Result<Response<TxHashes>, Status> {
        self.inner.read().await.find_unknown_transactions(request)
    }

    /// Insert the given transactions `txs` into this transaction pool.
    pub async fn insert_transactions(
        &mut self,
        txs: Vec<Tx>,
    ) -> Vec<ImportResult> {
        self.inner.write().await.insert_transactions(txs).await
    }

    /// Decode the given transactions, and insert them into this transaction
    /// pool.
    pub async fn import_transactions(
        &self,
        request: Request<ImportRequest>,
    ) -> Result<Response<ImportReply>, Status> {
        self.inner.write().await.import_transactions(request).await
    }

    /// Given a set of transaction hashes, return the full transaction details.
    pub async fn get_transactions(
        &self,
        request: Request<GetTransactionsRequest>,
    ) -> Result<Response<GetTransactionsReply>, Status> {
        self.inner.read().await.get_transactions(request)
    }
}

impl server::Txpool for TxPool {
    fn find_unknown_transactions<'a, 'async_trait>(
        &'a self,
        request: Request<TxHashes>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Response<TxHashes>, Status>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'a: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(self.find_unknown_transactions(request))
    }

    fn import_transactions<'a, 'async_trait>(
        &'a self,
        request: Request<ImportRequest>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Response<ImportReply>, Status>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'a: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(self.import_transactions(request))
    }

    fn get_transactions<'a, 'async_trait>(
        &'a self,
        request: Request<GetTransactionsRequest>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Response<GetTransactionsReply>, Status>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'a: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(self.get_transactions(request))
    }
}

#[cfg(test)]
mod tests {
    use crate::control::tests::TestControl;

    use std::convert::TryInto;

    use super::*;

    use turbo_proto::txpool::AccountInfo;

    fn inner() -> Inner<TestControl> {
        let ctrl = TestControl::new();
        let cfg = Config::builder().max_txs(10).control(String::new()).build();
        let mut out = Inner::with_config(ctrl, cfg);
        out.latest_block = Some(Default::default());
        out
    }

    fn vtx(gas_price: u64) -> VerifiedTx {
        tx(gas_price).try_into().unwrap()
    }

    fn tx(gas_price: u64) -> Tx {
        Tx {
            gas_price: gas_price.into(),
            gas_limit: Default::default(),
            nonce: 0,
            to: Default::default(),
            input: Default::default(),
            v: Default::default(),
            r: 1.into(),
            s: 1.into(),
            value: Default::default(),
        }
        .into()
    }

    #[test]
    fn cheapest() {
        let mut txs = Slab::new();
        let k0 = txs.insert(vtx(100));
        let k1 = txs.insert(vtx(101));

        let mut by_hash = HashMap::new();
        by_hash.insert(*txs[k0].hash(), k0);
        by_hash.insert(*txs[k1].hash(), k1);

        let mut by_price = BTreeSet::new();
        by_price.insert(Priced {
            key: k0,
            gas_price: *txs[k0].gas_price(),
        });
        by_price.insert(Priced {
            key: k1,
            gas_price: *txs[k1].gas_price(),
        });

        let inner = Inner {
            latest_block: Some(Default::default()),
            txs,
            by_hash,
            by_price,
            control: TestControl::new(),
            max_txs: 2,
        };

        assert_eq!(inner.cheapest_key(), Some(k0));
        assert_eq!(inner.cheapest(), Some(&inner.txs[0]));
    }

    #[tokio::test]
    async fn insert_2() {
        let mut inner = inner();
        let tx0 = vtx(100);
        let tx1 = vtx(101);

        let q0 = inner.qualify(tx0.tx().clone()).await.unwrap();
        inner.insert(q0).unwrap();

        let q1 = inner.qualify(tx1.tx().clone()).await.unwrap();
        inner.insert(q1).unwrap();

        assert_eq!(inner.txs.len(), 2);
        assert_eq!(inner.by_hash.len(), 2);
        assert_eq!(inner.by_price.len(), 2);

        assert_eq!(inner.txs[inner.by_hash[tx0.hash()]], tx0);
        assert_eq!(inner.txs[inner.by_hash[tx1.hash()]], tx1);

        let mut iter = inner.by_price.iter();
        let i0 = iter.next().unwrap();
        let i1 = iter.next().unwrap();

        assert_eq!(inner.txs[i0.key], tx0);
        assert_eq!(i0.gas_price, *tx0.gas_price());

        assert_eq!(inner.txs[i1.key], tx1);
        assert_eq!(i1.gas_price, *tx1.gas_price());
    }

    #[tokio::test]
    async fn qualify_fee_too_low() {
        let mut inner = Inner::with_config(
            TestControl::new(),
            Config::builder().control("").max_txs(1).build(),
        );
        inner.latest_block = Some(Default::default());

        let q0 = inner.qualify(tx(100)).await.unwrap();
        inner.insert(q0).unwrap();

        let r1 = inner.qualify(tx(99)).await.unwrap_err();
        assert!(matches!(r1, ImportError::FeeTooLow { .. }));
    }

    #[tokio::test]
    async fn insert_already_exists() {
        let mut inner = inner();

        let q0 = inner.qualify(tx(100)).await.unwrap();
        inner.insert(q0).unwrap();

        let q1 = inner.qualify(tx(100)).await.unwrap();
        let r1 = inner.insert(q1).unwrap_err();
        assert!(matches!(r1, ImportError::AlreadyExists { .. }));
    }

    #[tokio::test]
    async fn insert_evict() {
        let mut inner = Inner::with_config(
            TestControl::new(),
            Config::builder().control("").max_txs(1).build(),
        );
        inner.latest_block = Some(Default::default());

        let tx0 = tx(99);
        let q0 = inner.qualify(tx0).await.unwrap();
        inner.insert(q0).unwrap();

        let tx1 = vtx(100);
        let q1 = inner.qualify(tx1.tx().clone()).await.unwrap();
        inner.insert(q1).unwrap();

        assert_eq!(inner.txs.len(), 1);
        assert_eq!(inner.by_hash.len(), 1);
        assert_eq!(inner.by_price.len(), 1);

        assert_eq!(inner.txs[inner.by_hash[tx1.hash()]], tx1);

        let mut iter = inner.by_price.iter();
        let i0 = iter.next().unwrap();

        assert_eq!(inner.txs[i0.key], tx1);
        assert_eq!(i0.gas_price, *tx1.gas_price());
    }

    #[tokio::test]
    async fn qualify_future_nonce() {
        let mut inner = Inner::with_config(
            TestControl::new(),
            Config::builder().control("").max_txs(1).build(),
        );
        inner.latest_block = Some(Default::default());

        let tx0 = Tx {
            gas_price: 1.into(),
            gas_limit: Default::default(),
            nonce: 10,
            to: Default::default(),
            input: Default::default(),
            v: Default::default(),
            r: 1.into(),
            s: 1.into(),
            value: Default::default(),
        };

        let q0 = inner.qualify(tx0).await.unwrap_err();
        assert!(matches!(q0, ImportError::InvalidNonce { .. }));
    }

    #[tokio::test]
    async fn qualify_insufficient_balance() {
        let mut inner = Inner::with_config(
            TestControl::new(),
            Config::builder().control("").max_txs(1).build(),
        );
        inner.latest_block = Some(Default::default());

        let tx0 = Tx {
            gas_price: 1.into(),
            gas_limit: Default::default(),
            nonce: 0,
            to: Default::default(),
            input: Default::default(),
            v: Default::default(),
            r: 1.into(),
            s: 1.into(),
            value: 0xFFFFFFFFFFFFFFFFFu128.into(),
        };

        let q0 = inner.qualify(tx0).await.unwrap_err();
        assert!(matches!(q0, ImportError::InsufficientBalance { .. }));
    }

    #[tokio::test]
    async fn applied_block_conflicts() {
        let ctrl = TestControl::new();

        let mut inner = Inner::with_config(
            ctrl.clone(),
            Config::builder().control("").max_txs(1).build(),
        );
        inner.latest_block = Some(Default::default());

        let tx0 = vtx(100);
        let inserted = inner.insert_transactions(vec![tx0.tx().clone()]).await;
        assert_eq!(inserted, &[ImportResult::Success]);
        assert_eq!(inner.txs.len(), 1);

        let mut nonce = [0u8; 32];
        U256::one().to_big_endian(&mut nonce);

        let account = AccountInfo {
            address: tx0.from().as_bytes().to_vec(),
            balance: TestControl::BALANCE.to_vec(),
            nonce: nonce.to_vec(),
        };

        let account_diff = AccountDiff {
            diff: Some(account_diff::Diff::Changed(account)),
        };

        let applied = AppliedBlock {
            hash: vec![0u8; 32],
            parent_hash: vec![1u8; 32],
            account_diffs: vec![account_diff],
        };

        let diff = BlockDiff {
            diff: Some(block_diff::Diff::Applied(applied)),
        };

        inner.block_diff(diff).await;

        assert!(inner.txs.is_empty());
    }

    #[tokio::test]
    async fn applied_block_no_conflicts() {
        let ctrl = TestControl::new();

        let mut inner = Inner::with_config(
            ctrl.clone(),
            Config::builder().control("").max_txs(1).build(),
        );
        inner.latest_block = Some(Default::default());

        let tx0 = vtx(100);
        let inserted = inner.insert_transactions(vec![tx0.tx().clone()]).await;
        assert_eq!(inserted, &[ImportResult::Success]);
        assert_eq!(inner.txs.len(), 1);

        let mut nonce = [0u8; 32];
        U256::one().to_big_endian(&mut nonce);

        let account = AccountInfo {
            address: vec![0u8; 20],
            balance: TestControl::BALANCE.to_vec(),
            nonce: nonce.to_vec(),
        };

        let account_diff = AccountDiff {
            diff: Some(account_diff::Diff::Changed(account)),
        };

        let applied = AppliedBlock {
            hash: vec![0u8; 32],
            parent_hash: vec![1u8; 32],
            account_diffs: vec![account_diff],
        };

        let diff = BlockDiff {
            diff: Some(block_diff::Diff::Applied(applied)),
        };

        inner.block_diff(diff).await;

        assert_eq!(inner.txs.len(), 1);
    }
}
