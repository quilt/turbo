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

//! Error types for the transaction pool.

pub(crate) mod decode_error {
    use snafu::{ResultExt, Snafu};

    use turbo_proto::txpool::ImportResult;

    /// Errors possible while decoding transactions.
    #[derive(Debug, Snafu)]
    #[non_exhaustive]
    #[snafu(visibility = "pub(crate)")]
    pub enum DecodeError {
        /// Decoding a structure from RLP failed.
        RlpDecode {
            /// The preceding cause of the failure.
            source: Box<dyn std::error::Error + Send + Sync>,

            /// The field that failed to decode.
            field: Option<&'static str>,
        },

        /// An integer was out of the range of the transaction pool's internal
        /// representation.
        IntegerOverflow,
    }

    pub(crate) trait RlpResultExt<T> {
        fn context_field(self, field: &'static str) -> Result<T, DecodeError>;
    }

    impl<T> RlpResultExt<T> for Result<T, rlp::DecoderError> {
        fn context_field(self, field: &'static str) -> Result<T, DecodeError> {
            self.map_err(|e| Box::new(e).into())
                .context(RlpDecode { field })
        }
    }

    impl From<DecodeError> for ImportResult {
        fn from(e: DecodeError) -> ImportResult {
            match e {
                DecodeError::RlpDecode { .. } => ImportResult::Invalid,
                DecodeError::IntegerOverflow => ImportResult::InternalError,
            }
        }
    }
}

pub(crate) mod import_error {
    use ethereum_types::{Address, H256, U256};

    use snafu::Snafu;

    use turbo_proto::txpool::ImportResult;

    /// Errors that can occur while importing transactions into the pool.
    #[derive(Debug, Snafu)]
    #[non_exhaustive]
    #[snafu(visibility = "pub(crate)")]
    pub enum ImportError {
        /// The nonce provided with the transaction has already been used.
        NonceUsed {
            /// The offending transaction hash.
            tx_hash: H256,
        },

        /// The nonce provided with the transaction is too far in the future.
        #[snafu(display(
            "nonce is too far in the future tx_nonce={} from={} tx={}",
            from,
            tx_nonce,
            tx_hash,
        ))]
        NonceGap {
            /// Account that signed the transaction
            from: Address,
            /// The transaction's nonce.
            tx_nonce: u64,
            /// The offending transaction hash.
            tx_hash: H256,
        },

        /// The account's balance is insufficient to cover the gas fees and/or
        /// value sent with the transaction.
        InsufficientBalance {
            /// The offending transaction hash.
            tx_hash: H256,
        },

        /// The transaction pool has not yet received a block from the control
        /// interface.
        NotReady,

        /// The transaction pool is at capacity, and the transaction's fee was
        /// too low to evict an existing transaction.
        FeeTooLow {
            /// The gas price of the lowest paying transaction in the pool.
            minimum: U256,
        },

        /// The signature for the transaction could not be verified.
        Ecdsa {
            /// The preceding cause of this error.
            source: Box<dyn std::error::Error + Send + Sync>,
        },

        /// The transaction failed to decode.
        #[snafu(context(false))]
        Decode {
            /// The preceding cause of this error.
            source: super::DecodeError,
        },

        /// The transaction already exists in the pool.
        AlreadyExists {
            /// The offending transaction hash.
            tx_hash: H256,
        },

        /// Request to backend failed.
        RequestFailed {
            /// The preceding cause of this error.
            source: tonic::Status,
        },
    }

    impl From<ImportError> for ImportResult {
        fn from(e: ImportError) -> ImportResult {
            match e {
                ImportError::NonceGap { .. } => ImportResult::Invalid,
                ImportError::NonceUsed { .. } => ImportResult::Invalid,
                ImportError::FeeTooLow { .. } => ImportResult::FeeTooLow,
                ImportError::NotReady => ImportResult::InternalError,
                ImportError::Ecdsa { .. } => ImportResult::Invalid,
                ImportError::Decode { .. } => ImportResult::Invalid,
                ImportError::AlreadyExists { .. } => {
                    ImportResult::AlreadyExists
                }
                ImportError::RequestFailed { .. } => {
                    ImportResult::InternalError
                }
                ImportError::InsufficientBalance { .. } => {
                    ImportResult::Invalid
                }
            }
        }
    }
}

pub use self::decode_error::DecodeError;
pub use self::import_error::ImportError;
