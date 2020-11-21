use crate::{grpc::txpool::txpool_server::TxpoolServer, txpool::TxpoolService};
use ethereum_txpool::Pool;
use ethereum_types::U256;
use std::{sync::Arc, time::Duration};
use task_group::TaskGroup;
use tokio::{sync::Mutex as AsyncMutex, time::sleep};
use tokio_compat_02::*;
use tonic::transport::Server;
use tracing::*;

mod data_provider;
mod grpc;
mod txpool;

async fn real_main() {
    let tasks = Arc::new(TaskGroup::new());

    let web3_addr = "127.0.0.1:5455";
    let grpc_server_addr = "127.0.0.1:8080".parse().unwrap();

    let data_provider = data_provider::Web3DataProvider::new(web3_addr).unwrap();

    let pool = Arc::new(AsyncMutex::new(Pool::new()));

    tasks.spawn({
        let pool = pool.clone();
        async move {
            let svc = TxpoolServer::new(TxpoolService::new(
                pool,
                data_provider,
                U256::from(10_000_000_000_u64),
            ));

            info!("Sentry gRPC server starting on {}", grpc_server_addr);

            Server::builder()
                .add_service(svc)
                .serve(grpc_server_addr)
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
