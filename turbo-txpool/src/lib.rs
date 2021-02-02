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

use std::cmp::Reverse;
use std::collections::{hash_map, BTreeSet, BinaryHeap, HashMap};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use tokio_stream::StreamExt;

use tonic::transport::Server;
use tonic::{Request, Response, Status};

use tracing::{debug, info};

use turbo_proto::txpool::block_stream_request::StartWith;
use turbo_proto::txpool::txpool_control_client as client;
use turbo_proto::txpool::txpool_server as server;
use turbo_proto::txpool::{
    account_diff, block_diff, AccountDiff, AccountInfoRequest, AppliedBlock,
    BlockDiff, BlockStreamRequest, GetTransactionsReply,
    GetTransactionsRequest, ImportReply, ImportRequest, ImportResult,
    RevertedBlock, TxHashes,
};

#[derive(Debug, Clone, Eq, PartialEq)]
struct PooledTx {
    tx: VerifiedTx,
    runnable: bool,
}

#[derive(Debug, Ord, PartialOrd, Eq, PartialEq)]
struct Priced {
    pub gas_price: U256,
    pub key: usize,
}

#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq)]
struct Nonced {
    pub nonce: u64,
    pub key: usize,
}

#[derive(Debug)]
struct Inner<C> {
    control: C,
    txs: Slab<PooledTx>,
    by_hash: HashMap<H256, usize>,
    by_price: BTreeSet<Priced>,

    soon: HashMap<Address, BTreeSet<Nonced>>,

    latest_block: Option<H256>,
    max_txs: usize,
}

impl<C> Inner<C> {
    fn cheapest_key(&self) -> Option<usize> {
        self.by_price.iter().next().map(|p| p.key)
    }

    fn cheapest(&self) -> Option<&PooledTx> {
        self.cheapest_key().map(|k| &self.txs[k])
    }

    fn by_hash(&self, hash: &H256) -> Option<&PooledTx> {
        self.by_hash.get(hash).and_then(|k| self.txs.get(*k))
    }

    fn remove_soon(
        soon: &mut HashMap<Address, BTreeSet<Nonced>>,
        account: Address,
        nonced: &Nonced,
    ) -> bool {
        let mut entry = match soon.entry(account) {
            hash_map::Entry::Vacant(_) => return false,
            hash_map::Entry::Occupied(o) => o,
        };

        if !entry.get_mut().remove(&nonced) {
            return false;
        }

        if entry.get().is_empty() {
            entry.remove();
        }

        true
    }

    fn remove(&mut self, key: usize) -> PooledTx {
        let ptx = self.txs.remove(key);
        self.by_hash.remove(ptx.tx.hash()).expect("desync by hash");

        if ptx.runnable {
            let removed = self.by_price.remove(&Priced {
                gas_price: *ptx.tx.gas_price(),
                key,
            });
            assert!(removed, "desync by price");
        } else {
            let nonced = Nonced {
                key,
                nonce: ptx.tx.nonce(),
            };

            let removed =
                Self::remove_soon(&mut self.soon, *ptx.tx.from(), &nonced);
            assert!(removed, "desync in soon");
        }

        ptx
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
            soon: HashMap::with_capacity(config.max_txs()),
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
            .map(|ptx| {
                let mut stream = rlp::RlpStream::new();
                ptx.tx.tx().encode(&mut stream);
                stream.as_raw().to_vec()
            })
            .collect();

        Ok(Response::new(GetTransactionsReply { txs }))
    }

    fn evict_soon(&mut self) {
        // Build a min-heap from the highest nonce'd transactions in each
        // account.
        let mut heap: BinaryHeap<_> = self
            .soon
            .values()
            .flat_map(|x| x.iter().next_back())
            .map(|nonced| Priced {
                gas_price: *self.txs[nonced.key].tx.gas_price(),
                key: nonced.key,
            })
            .map(Reverse)
            .collect();

        while self.txs.len() > self.max_txs {
            // Get the cheapest "soon" transaction (or break)
            let popped = match heap.pop() {
                Some(p) => p,
                None => break,
            };

            // Remove that transaction, but save its account.
            let ptx = self.remove(popped.0.key);
            let account = *ptx.tx.from();
            debug!(
                "Evicting soon tx from={} nonce={} price={} hash={}",
                account,
                ptx.tx.nonce(),
                ptx.tx.gas_price(),
                ptx.tx.hash(),
            );

            // Get transaction with the preceeding nonce from the same account.
            let next_nonced =
                self.soon.get(&account).and_then(|t| t.iter().next_back());
            let next_nonced = match next_nonced {
                Some(nn) => nn,
                None => continue,
            };

            let next_priced = Priced {
                key: next_nonced.key,
                gas_price: *self.txs[next_nonced.key].tx.gas_price(),
            };

            // Add that preceeding transaction into the heap, and evict again!
            heap.push(Reverse(next_priced));
        }
    }

    fn log_insert(&mut self, pooled: PooledTx) -> Result<(), ImportError> {
        let result = self.insert(pooled.clone());
        match &result {
            Err(e) => debug!(
                "Rejected tx from={} nonce={} price={} hash={} reason={}",
                pooled.tx.from(),
                pooled.tx.nonce(),
                pooled.tx.gas_price(),
                pooled.tx.hash(),
                e,
            ),
            Ok(_) => debug!(
                "Inserted tx from={} nonce={} price={} hash={}",
                pooled.tx.from(),
                pooled.tx.nonce(),
                pooled.tx.gas_price(),
                pooled.tx.hash(),
            ),
        }

        result
    }

    fn insert(&mut self, pooled: PooledTx) -> Result<(), ImportError> {
        let by_hash = match self.by_hash.entry(*pooled.tx.hash()) {
            hash_map::Entry::Vacant(v) => v,
            hash_map::Entry::Occupied(_) => {
                return Err(ImportError::AlreadyExists {
                    tx_hash: *pooled.tx.hash(),
                })
            }
        };

        let key = self.txs.insert(pooled.clone());

        if pooled.runnable {
            let inserted = self.by_price.insert(Priced {
                gas_price: *pooled.tx.gas_price(),
                key,
            });
            assert!(inserted);
            by_hash.insert(key);
        } else {
            let set = self.soon.entry(*pooled.tx.from()).or_default();

            let contiguous_end = set
                .iter()
                .next_back()
                .map(|nonced| (nonced.nonce + 1) == pooled.tx.nonce())
                .unwrap_or(true);

            let contiguous_start = set
                .iter()
                .next()
                .map(|nonced| (nonced.nonce - 1) == pooled.tx.nonce())
                .unwrap_or(true);

            let contiguous = contiguous_start | contiguous_end;

            // TODO: The above `contiguous` calculation is meant to prevent gaps
            //       between the current account nonce, the runnable tx (if one
            //       exists), and the set of "soon" txs.
            //
            //       Gaps between transactions make the eviction policy
            //       extremely unfair.
            //
            //       The `contiguous` calculation, so far, only detects gaps in
            //       the "soon" tx set, and not between the current nonce or
            //       the next runnable tx, so the eviction is unfair.

            ensure!(
                contiguous,
                import_error::NonceGap {
                    from: *pooled.tx.from(),
                    tx_hash: *pooled.tx.hash(),
                    tx_nonce: pooled.tx.nonce(),
                }
            );

            let inserted = set.insert(Nonced {
                nonce: pooled.tx.nonce(),
                key,
            });
            assert!(inserted);

            by_hash.insert(key);
        };

        self.evict_soon();

        while self.txs.len() > self.max_txs {
            let ptx = self.remove(self.cheapest_key().unwrap());
            debug!(
                "Evicting cheapest tx from={} nonce={} price={} tx={}",
                ptx.tx.from(),
                ptx.tx.nonce(),
                ptx.tx.gas_price(),
                ptx.tx.hash(),
            );
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

        let by_price = &mut self.by_price;
        let soon = &mut self.soon;

        self.txs.retain(|key, ptx| {
            let account = match accounts.get(ptx.tx.from()) {
                Some(a) => a,
                None => return true,
            };

            match ptx.tx.is_runnable(account.nonce, account.balance) {
                Ok(_) => {
                    // TODO: Move newly runnable transactions from `soon` to
                    // `by_price`
                    return true;
                }
                Err(e) => {
                    debug!(
                        "Dropping tx from={} nonce={} price={} hash={} reason={}",
                        ptx.tx.from(),
                        ptx.tx.nonce(),
                        ptx.tx.gas_price(),
                        ptx.tx.hash(),
                        e
                    );
                },
            }

            by_price.remove(&Priced {
                key,
                gas_price: *ptx.tx.gas_price(),
            });

            let nonced = Nonced {
                key,
                nonce: ptx.tx.nonce(),
            };
            Self::remove_soon(soon, *ptx.tx.from(), &nonced);

            false
        });
    }

    async fn block_reverted(&mut self, block: RevertedBlock) {
        self.latest_block = Some(H256::from_slice(&block.new_hash));

        let req = ImportRequest {
            txs: block.reverted_transactions,
        };

        info!("block reverted latest_block={}", self.latest_block.unwrap(),);

        self.recheck(&block.new_state);
        self.import_transactions_request(req).await.ok();
    }

    async fn block_applied(&mut self, block: AppliedBlock) {
        let hash = H256::from_slice(&block.hash);

        self.latest_block = Some(hash);
        info!("block applied latest_block={}", self.latest_block.unwrap(),);

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

    async fn log_qualify(&self, tx: Tx) -> Result<PooledTx, ImportError> {
        let result = self.qualify(tx).await;

        if let Err(ref e) = result {
            debug!("Disqualified transaction reason={}", e,);
        }

        result
    }

    async fn qualify(&self, tx: Tx) -> Result<PooledTx, ImportError> {
        let latest_block = self.latest_block.context(import_error::NotReady)?;

        if self.txs.len() >= self.max_txs {
            let cheapest = self
                .cheapest()
                .map(|t| *t.tx.gas_price())
                .unwrap_or_default();

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

        // TODO: Ensure the tx can actually fit in a block.

        let runnable = verified.is_runnable(nonce, balance)?;

        Ok(PooledTx {
            tx: verified,
            runnable,
        })
    }

    pub async fn insert_transactions(
        &mut self,
        txs: Vec<Tx>,
    ) -> Vec<ImportResult> {
        let verified: Vec<_> = futures_util::future::join_all(
            txs.into_iter().map(|tx| self.log_qualify(tx)),
        )
        .await;

        let mut result = Vec::with_capacity(verified.len());
        for res in verified.into_iter() {
            let ins = match res.and_then(|vx| self.log_insert(vx)) {
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

            async { self.log_qualify(decoded?).await }
        });

        let txs = futures_util::future::join_all(qualifiers).await;

        let mut imported = Vec::with_capacity(txs.len());

        for tx in txs.into_iter() {
            let result = match tx.and_then(|tx| self.log_insert(tx)) {
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

    fn ptx(gas_price: u64) -> PooledTx {
        PooledTx {
            runnable: true,
            tx: vtx(gas_price),
        }
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
        let k0 = txs.insert(ptx(100));
        let k1 = txs.insert(ptx(101));

        let mut by_hash = HashMap::new();
        by_hash.insert(*txs[k0].tx.hash(), k0);
        by_hash.insert(*txs[k1].tx.hash(), k1);

        let mut by_price = BTreeSet::new();
        by_price.insert(Priced {
            key: k0,
            gas_price: *txs[k0].tx.gas_price(),
        });
        by_price.insert(Priced {
            key: k1,
            gas_price: *txs[k1].tx.gas_price(),
        });

        let inner = Inner {
            latest_block: Some(Default::default()),
            soon: Default::default(),
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

        assert_eq!(inner.txs[inner.by_hash[tx0.hash()]].tx, tx0);
        assert_eq!(inner.txs[inner.by_hash[tx1.hash()]].tx, tx1);

        let mut iter = inner.by_price.iter();
        let i0 = iter.next().unwrap();
        let i1 = iter.next().unwrap();

        assert_eq!(inner.txs[i0.key].tx, tx0);
        assert_eq!(i0.gas_price, *tx0.gas_price());

        assert_eq!(inner.txs[i1.key].tx, tx1);
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

        assert_eq!(inner.txs[inner.by_hash[tx1.hash()]].tx, tx1);

        let mut iter = inner.by_price.iter();
        let i0 = iter.next().unwrap();

        assert_eq!(inner.txs[i0.key].tx, tx1);
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

        let q0 = inner.qualify(tx0.clone()).await.unwrap();
        assert_eq!(q0.tx.tx(), &tx0);
        assert_eq!(q0.runnable, false);
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

    #[tokio::test]
    async fn insert_soon_tx() {
        let mut inner = Inner::with_config(
            TestControl::new(),
            Config::builder().control("").max_txs(1).build(),
        );
        inner.latest_block = Some(Default::default());

        let mut tx0 = tx(100);
        tx0.nonce = 1;
        let vtx0 = VerifiedTx::new(tx0.clone()).unwrap();

        let inserted = inner.insert_transactions(vec![tx0]).await;
        assert_eq!(inserted, [ImportResult::Success]);

        assert_eq!(inner.txs.len(), 1);
        assert!(matches!(inner.txs[0], PooledTx {
            runnable: false,
            ..
        }));

        let soon = &inner.soon[vtx0.from()];
        assert_eq!(soon.len(), 1);

        let nonced = soon.iter().next().unwrap();
        assert_eq!(nonced.nonce, 1);
    }

    #[tokio::test]
    async fn reject_soon_tx() {
        let mut inner = Inner::with_config(
            TestControl::new(),
            Config::builder().control("").max_txs(1).build(),
        );
        inner.latest_block = Some(Default::default());

        let mut tx0 = tx(100);
        tx0.nonce = 1;

        inner.insert_transactions(vec![tx(1), tx0]).await;

        assert_eq!(inner.txs.len(), 1);
        assert_eq!(inner.txs[0].runnable, true);
        assert_eq!(inner.txs[0].tx.gas_price(), &1.into());

        assert_eq!(inner.soon.len(), 0);
    }

    #[tokio::test]
    async fn evict_soon_tx_with_runnable() {
        let mut inner = Inner::with_config(
            TestControl::new(),
            Config::builder().control("").max_txs(1).build(),
        );
        inner.latest_block = Some(Default::default());

        let mut tx0 = tx(100);
        tx0.nonce = 1;

        inner.insert_transactions(vec![tx0, tx(1)]).await;

        assert_eq!(inner.txs.len(), 1);
        assert_eq!(inner.txs[1].runnable, true);
        assert_eq!(inner.txs[1].tx.gas_price(), &1.into());

        assert!(inner.soon.is_empty());
        assert_eq!(inner.by_price.len(), 1);
    }

    #[tokio::test]
    async fn evict_soon_tx_with_soon_different_account() {
        let mut inner = Inner::with_config(
            TestControl::new(),
            Config::builder().control("").max_txs(1).build(),
        );
        inner.latest_block = Some(Default::default());

        let mut tx0 = tx(100);
        tx0.nonce = 1;

        let mut tx1 = tx(101);
        tx1.nonce = 1;

        inner.insert_transactions(vec![tx0, tx1.clone()]).await;

        assert_eq!(inner.txs.len(), 1);
        assert_eq!(inner.txs[1].runnable, false);
        assert_eq!(inner.txs[1].tx.tx(), &tx1);

        assert_eq!(inner.soon.len(), 1);
        assert!(inner.by_price.is_empty());
    }

    // TODO: Test multiple soon transactions from the same account
}
