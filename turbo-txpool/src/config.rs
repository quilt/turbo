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

use serde::{Deserialize, Serialize};

use typed_builder::TypedBuilder;

#[derive(Debug, TypedBuilder, Serialize, Deserialize)]
pub struct Config {
    max_txs: usize,

    #[builder(setter(into))]
    control: String,
}

impl Config {
    pub fn max_txs(&self) -> usize {
        self.max_txs
    }

    pub fn control(&self) -> &str {
        &self.control
    }
}
