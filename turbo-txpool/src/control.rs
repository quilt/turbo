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

use async_trait::async_trait;

use tonic::transport::Channel;
use tonic::{Status, Streaming};

use ethereum_interfaces::txpool::txpool_control_client::TxpoolControlClient;
use ethereum_interfaces::txpool::{
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
    use ethereum_types::U256;

    use hex_literal::hex;

    use super::*;

    use std::pin::Pin;

    use tokio::sync::broadcast::{self, Sender};

    #[derive(Clone, Debug)]
    pub struct TestControl {
        block_send: Sender<Result<BlockDiff, Status>>,
    }

    impl TestControl {
        pub const BALANCE: [u8; 32] = hex!(
            "000000000000000000000000000000000000000000000000FFFFFFFFFFFFFFFF"
        );

        pub fn new() -> Self {
            let (block_send, _) = broadcast::channel(1);
            Self { block_send }
        }

        #[allow(unused)] // TODO: Use this
        pub fn stream_block(&self, result: Result<BlockDiff, Status>) {
            self.block_send.send(result).unwrap();
        }
    }

    #[async_trait]
    impl Control for TestControl {
        type BlockStream = Pin<
            Box<
                dyn futures_core::stream::Stream<
                    Item = Result<BlockDiff, Status>,
                >,
            >,
        >;

        async fn block_stream(
            &mut self,
            _request: BlockStreamRequest,
        ) -> Result<Self::BlockStream, Status> {
            let recv = self.block_send.subscribe();
            let fut = futures_util::stream::unfold(recv, |mut f| async {
                f.recv().await.ok().map(|i| (i, f))
            });

            Ok(Box::pin(fut))
        }

        async fn account_info(
            &mut self,
            _request: AccountInfoRequest,
        ) -> Result<AccountInfoReply, Status> {
            let balance = U256::from(&Self::BALANCE);
            Ok(AccountInfoReply {
                nonce: Default::default(),
                balance: Some(balance.into()),
            })
        }
    }
}
