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

//! Crate containing the code for interacting with the turbo-geth gRPC
//! interfaces.

#![deny(unsafe_code, missing_debug_implementations)]

pub extern crate prost;

pub mod txpool {
    //! The gRPC interfaces related to the transaction pool. `txpool_client` and
    //! `txpool_server` expose the transaction pool's functionality, while
    //! `txpool_control_server` and `txpool_control_client` facilitate
    //! communication between the transaction pool and the backend.

    tonic::include_proto!("txpool");
    tonic::include_proto!("txpool_control");
}

pub mod p2psentry {
    //! The gRPC interfaces related to the p2p sentry.
    tonic::include_proto!("sentry");
}

pub mod snapshot_downloader {
    //! The gRPC interfaces related to the snapshot sync system.
    tonic::include_proto!("snapshotsync");
}
