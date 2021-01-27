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

//! Configuration definition for the transaction pool. Can be deserialized, or
//! built:
//!
//! ```rust
//! use turbo_txpool::config::Config;
//!
//! let config = Config::builder()
//!     .max_txs(1024)
//!     .control("http://127.0.0.1:9092")
//!     .build();
//! ```

use serde::{Deserialize, Serialize};

use typed_builder::TypedBuilder;

/// Configuration for the transaction pool. See the module documentation for an
/// example.
#[derive(Debug, TypedBuilder, Serialize, Deserialize)]
pub struct Config {
    max_txs: usize,

    #[builder(setter(into))]
    control: String,
}

impl Config {
    /// The maximum number of transactions to store in the pool at any time.
    pub fn max_txs(&self) -> usize {
        self.max_txs
    }

    /// The URL of the `txpool_control` server.
    pub fn control(&self) -> &str {
        &self.control
    }
}
