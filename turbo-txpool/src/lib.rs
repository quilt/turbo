use std::future::Future;
use std::pin::Pin;

use tonic::{Request, Response, Status};

use turbo_proto::tonic;
use turbo_proto::txpool::txpool_server as server;
use turbo_proto::txpool::{
    GetTransactionsReply, GetTransactionsRequest, ImportReply, ImportRequest,
    TxHashes,
};

pub struct TxPool {}

impl TxPool {
    pub async fn find_unknown_transactions(
        &self,
        _request: Request<TxHashes>,
    ) -> Result<Response<TxHashes>, Status> {
        todo!()
    }

    pub async fn import_transactions(
        &self,
        _request: Request<ImportRequest>,
    ) -> Result<Response<ImportReply>, Status> {
        todo!()
    }

    pub async fn get_transactions(
        &self,
        _request: Request<GetTransactionsRequest>,
    ) -> Result<Response<GetTransactionsReply>, Status> {
        todo!()
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
