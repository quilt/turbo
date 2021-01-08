use async_trait::async_trait;

use tonic::transport::Channel;
use tonic::{Status, Streaming};

use turbo_proto::txpool::txpool_control_client::TxpoolControlClient;
use turbo_proto::txpool::{
    AccountInfoReply, AccountInfoRequest, BlockDiff, BlockStreamRequest,
};

#[async_trait]
pub trait Control {
    type BlockStream: futures_core::stream::Stream<
        Item = Result<BlockDiff, Status>,
    >;

    async fn block_stream(
        &mut self,
        request: BlockStreamRequest,
    ) -> Result<Self::BlockStream, Status>;

    async fn account_info(
        &mut self,
        request: AccountInfoRequest,
    ) -> Result<AccountInfoReply, Status>;
}

#[derive(Debug)]
pub struct PbControl {
    client: TxpoolControlClient<Channel>,
}

#[async_trait]
impl Control for PbControl {
    type BlockStream = Streaming<BlockDiff>;

    async fn block_stream(
        &mut self,
        request: BlockStreamRequest,
    ) -> Result<Self::BlockStream, Status> {
        self.client
            .block_stream(request)
            .await
            .map(|r| r.into_inner())
    }

    async fn account_info(
        &mut self,
        request: AccountInfoRequest,
    ) -> Result<AccountInfoReply, Status> {
        self.client
            .account_info(request)
            .await
            .map(|r| r.into_inner())
    }
}

impl PbControl {
    pub fn new(client: TxpoolControlClient<Channel>) -> Self {
        Self { client }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[derive(Debug)]
    pub struct TestControl;

    #[async_trait]
    impl Control for TestControl {
        type BlockStream = tokio::stream::Pending<Result<BlockDiff, Status>>;

        async fn block_stream(
            &mut self,
            request: BlockStreamRequest,
        ) -> Result<Self::BlockStream, Status> {
            Ok(tokio::stream::pending())
        }

        async fn account_info(
            &mut self,
            request: AccountInfoRequest,
        ) -> Result<AccountInfoReply, Status> {
            Err(Status::not_found("TODO"))
        }
    }
}
