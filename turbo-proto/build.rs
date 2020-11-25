fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure().compile(
        &[
            "interfaces/txpool/txpool.proto",
            "interfaces/txpool/txpool_control.proto",
            "interfaces/p2psentry/control.proto",
            "interfaces/p2psentry/sentry.proto",
            "interfaces/snapshot_downloader/external_downloader.proto",
        ],
        &["interfaces"],
    )?;

    Ok(())
}
