pub extern crate prost;
pub extern crate tonic;

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
