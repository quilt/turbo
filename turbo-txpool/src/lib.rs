mod control;
pub mod error;
pub mod tx;

#[cfg(feature = "arbitrary")]
extern crate arbitrary_dep as arbitrary;

use crate::control::{Control, PbControl};
use crate::error::Error;
use crate::tx::{Tx, VerifiedTx};

use ethereum_types::{H256, U256};

use slab::Slab;

use std::collections::{hash_map, BTreeSet, HashMap};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;

use tokio::sync::RwLock;

use tonic::transport::Server;
use tonic::{Request, Response, Status};

use turbo_proto::txpool::txpool_control_client as client;
use turbo_proto::txpool::txpool_server as server;
pub use turbo_proto::txpool::ImportResult;
use turbo_proto::txpool::{
    GetTransactionsReply, GetTransactionsRequest, ImportReply, ImportRequest,
    TxHashes,
};

use typed_builder::TypedBuilder;

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
            max_txs: config.max_txs,
            txs: Slab::with_capacity(config.max_txs),
            by_hash: HashMap::with_capacity(config.max_txs),
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
                stream.drain()
            })
            .collect();

        Ok(Response::new(GetTransactionsReply { txs }))
    }
}

impl<C> Inner<C>
where
    C: Control,
{
    fn insert(&mut self, tx: Tx) -> ImportResult {
        if self.txs.len() >= self.max_txs {
            let cheapest =
                self.cheapest().map(|t| *t.gas_price()).unwrap_or_default();
            if tx.gas_price <= cheapest {
                return ImportResult::FeeTooLow;
            }
        }

        let verified = match VerifiedTx::new(tx) {
            Ok(v) => v,
            Err(_) => return ImportResult::Invalid,
        };

        let by_hash = match self.by_hash.entry(*verified.hash()) {
            hash_map::Entry::Vacant(v) => v,
            hash_map::Entry::Occupied(_) => return ImportResult::AlreadyExists,
        };

        let gas_price = *verified.gas_price();
        let key = self.txs.insert(verified);

        let inserted = self.by_price.insert(Priced { gas_price, key });
        assert!(inserted);

        by_hash.insert(key);

        while self.txs.len() > self.max_txs {
            self.remove(self.cheapest_key().unwrap());
        }

        ImportResult::Success
    }

    pub fn insert_transactions(&mut self, txs: Vec<Tx>) -> Vec<ImportResult> {
        txs.into_iter().map(|tx| self.insert(tx)).collect()
    }

    pub fn import_transactions(
        &mut self,
        request: Request<ImportRequest>,
    ) -> Result<Response<ImportReply>, Status> {
        let txs: Vec<_> = request
            .into_inner()
            .txs
            .into_iter()
            .map(|b| Tx::decode(&rlp::Rlp::new(&b)))
            .collect();

        let mut imported = Vec::with_capacity(txs.len());

        for tx in txs.into_iter() {
            let result = match tx {
                Ok(tx) => self.insert(tx),
                Err(Error::RlpDecode { .. }) => ImportResult::Invalid,
                Err(Error::IntegerOverflow) => ImportResult::InternalError,
            };
            imported.push(result as i32);
        }

        Ok(Response::new(ImportReply { imported }))
    }
}

#[derive(Debug, TypedBuilder)]
pub struct Config {
    max_txs: usize,
    #[builder(setter(into))]
    control: String,
}

pub struct TxPool {
    inner: RwLock<Inner<PbControl>>,
}

impl TxPool {
    pub async fn with_config(
        config: Config,
    ) -> Result<Self, tonic::transport::Error> {
        let client =
            client::TxpoolControlClient::connect(config.control.clone())
                .await?;
        let control = PbControl::new(client);
        Ok(Self {
            inner: RwLock::new(Inner::with_config(control, config)),
        })
    }

    pub async fn run<I>(self, addr: I) -> Result<(), tonic::transport::Error>
    where
        I: Into<SocketAddr>,
    {
        Server::builder()
            .add_service(server::TxpoolServer::new(self))
            .serve(addr.into())
            .await
    }

    pub async fn find_unknown_transactions(
        &self,
        request: Request<TxHashes>,
    ) -> Result<Response<TxHashes>, Status> {
        self.inner.read().await.find_unknown_transactions(request)
    }

    pub async fn insert_transactions(
        &mut self,
        txs: Vec<Tx>,
    ) -> Vec<ImportResult> {
        self.inner.write().await.insert_transactions(txs)
    }

    pub async fn import_transactions(
        &self,
        request: Request<ImportRequest>,
    ) -> Result<Response<ImportReply>, Status> {
        self.inner.write().await.import_transactions(request)
    }

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

    fn inner() -> Inner<TestControl> {
        let cfg = Config::builder().max_txs(10).control(String::new()).build();
        Inner::with_config(TestControl, cfg)
    }

    fn vtx(gas_price: u64) -> VerifiedTx {
        tx(gas_price).try_into().unwrap()
    }

    fn tx(gas_price: u64) -> Tx {
        Tx {
            gas_price: gas_price.into(),
            gas_limit: Default::default(),
            nonce: Default::default(),
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
            txs,
            by_hash,
            by_price,
            control: TestControl,
            max_txs: 2,
        };

        assert_eq!(inner.cheapest_key(), Some(k0));
        assert_eq!(inner.cheapest(), Some(&inner.txs[0]));
    }

    #[test]
    fn insert_2() {
        let mut inner = inner();
        let tx0 = vtx(100);
        let tx1 = vtx(101);

        let result = inner.insert(tx0.tx().clone());
        assert_eq!(result, ImportResult::Success);

        let result2 = inner.insert(tx1.tx().clone());
        assert_eq!(result2, ImportResult::Success);

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

    #[test]
    fn insert_fee_too_low() {
        let mut inner = Inner::with_config(
            TestControl,
            Config::builder().control("").max_txs(1).build(),
        );

        let r0 = inner.insert(tx(100));
        assert_eq!(r0, ImportResult::Success);

        let r1 = inner.insert(tx(99));
        assert_eq!(r1, ImportResult::FeeTooLow);
    }

    #[test]
    fn insert_already_exists() {
        let mut inner = inner();

        let r0 = inner.insert(tx(100));
        assert_eq!(r0, ImportResult::Success);

        let r1 = inner.insert(tx(100));
        assert_eq!(r1, ImportResult::AlreadyExists);
    }

    #[test]
    fn insert_evict() {
        let mut inner = Inner::with_config(
            TestControl,
            Config::builder().control("").max_txs(1).build(),
        );

        let tx0 = tx(99);
        let r0 = inner.insert(tx0.clone());
        assert_eq!(r0, ImportResult::Success);

        let tx1 = vtx(100);
        let r1 = inner.insert(tx1.tx().clone());
        assert_eq!(r1, ImportResult::Success);

        assert_eq!(inner.txs.len(), 1);
        assert_eq!(inner.by_hash.len(), 1);
        assert_eq!(inner.by_price.len(), 1);

        assert_eq!(inner.txs[inner.by_hash[tx1.hash()]], tx1);

        let mut iter = inner.by_price.iter();
        let i0 = iter.next().unwrap();

        assert_eq!(inner.txs[i0.key], tx1);
        assert_eq!(i0.gas_price, *tx1.gas_price());
    }
}
