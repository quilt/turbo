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

#[derive(Debug, Clone)]
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
    use hex_literal::hex;

    use super::*;

    #[derive(Clone, Debug)]
    pub struct TestControl;

    const BALANCE: [u8; 32] = hex!(
        "000000000000000000000000000000000000000000000000FFFFFFFFFFFFFFFF"
    );

    #[async_trait]
    impl Control for TestControl {
        type BlockStream = tokio_stream::Pending<Result<BlockDiff, Status>>;

        async fn block_stream(
            &mut self,
            _request: BlockStreamRequest,
        ) -> Result<Self::BlockStream, Status> {
            Ok(tokio_stream::pending())
        }

        async fn account_info(
            &mut self,
            _request: AccountInfoRequest,
        ) -> Result<AccountInfoReply, Status> {
            Ok(AccountInfoReply {
                nonce: Default::default(),
                balance: BALANCE.into(),
            })
        }
    }
}
