use crate::{
    config::*, data_provider::*, grpc::txpool::txpool_server::TxpoolServer, txpool::TxpoolService,
};
use clap::Clap;
use ethereum_txpool::Pool;
use ethereum_types::U256;
use std::{sync::Arc, time::Duration};
use task_group::TaskGroup;
use tokio::{sync::Mutex as AsyncMutex, time::sleep};
use tokio_compat_02::*;
use tonic::transport::Server;
use tracing::*;
use tracing_subscriber::EnvFilter;

mod config;
mod data_provider;
mod grpc;
mod txpool;

async fn real_main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let opts =
        toml::from_str::<Config>(&std::fs::read_to_string(Opts::parse().config_path).unwrap())
            .unwrap();

    let tasks = Arc::new(TaskGroup::new());

    let data_provider: Arc<dyn AccountInfoProvider> = match opts.data_provider {
        config::DataProvider::Web3 { addr } => Arc::new(Web3DataProvider::new(addr).unwrap()),
        config::DataProvider::Grpc { addr } => {
            Arc::new(GrpcDataProvider::connect(addr).await.unwrap())
        }
    };

    let listen_addr = opts.listen_addr.parse().unwrap();

    let pool = Arc::new(AsyncMutex::new(Pool::new()));

    tasks.spawn({
        let pool = pool.clone();
        async move {
            let svc = TxpoolServer::new(TxpoolService::new(
                pool,
                data_provider,
                U256::from(10_000_000_000_u64),
            ));

            info!("Sentry gRPC server starting on {}", listen_addr);

            Server::builder()
                .add_service(svc)
                .serve(listen_addr)
                .await
                .unwrap();
        }
    });

    loop {
        info!("Txpool: {:?}", pool.lock().await.status());

        sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::main]
async fn main() {
    real_main().compat().await
}
