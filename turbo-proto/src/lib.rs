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

pub extern crate prost;

pub mod txpool {
    tonic::include_proto!("txpool");
    tonic::include_proto!("txpool_control");
}

pub mod p2psentry {
    tonic::include_proto!("control");
    tonic::include_proto!("sentry");
}

pub mod snapshot_downloader {
    tonic::include_proto!("snapshotsync");
}
