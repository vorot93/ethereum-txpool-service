use clap::Clap;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clap)]
#[clap(
    name = "ethereum-txpool-service",
    about = "Service that listens to Ethereum's P2P network, serves information to other nodes, and provides gRPC interface to clients to interact with the network."
)]
pub struct Opts {
    #[clap(long, env)]
    pub config_path: PathBuf,
}

#[derive(Deserialize)]
#[serde(tag = "kind")]
pub enum DataProvider {
    Tarpc { addr: String },
    Control { addr: String },
}

#[derive(Deserialize)]
pub struct Config {
    pub listen_addr: String,
    pub data_provider: DataProvider,
}
