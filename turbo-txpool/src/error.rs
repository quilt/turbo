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

use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
#[snafu(visibility = "pub(crate)")]
pub enum Error {
    RlpDecode {
        source: rlp::DecoderError,
        field: Option<&'static str>,
    },
    IntegerOverflow,
}

pub(crate) trait RlpResultExt<T> {
    fn context_field(self, field: &'static str) -> Result<T, Error>;
}

impl<T> RlpResultExt<T> for Result<T, rlp::DecoderError> {
    fn context_field(self, field: &'static str) -> Result<T, Error> {
        self.context(RlpDecode { field })
    }
}
