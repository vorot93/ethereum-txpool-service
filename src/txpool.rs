use crate::grpc::txpool::{self, txpool_server::Txpool, *};
use async_trait::async_trait;
use auto_impl::auto_impl;
use ethereum::Transaction;
use ethereum_txpool::{AccountInfoProvider, ImportError, Pool};
use ethereum_types::*;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use txpool::TxHashes;

#[auto_impl(&, Box, Arc)]
pub trait GasPricer: Send + Sync + 'static {
    fn minimum_gas_price(&self) -> U256;
}

impl GasPricer for U256 {
    fn minimum_gas_price(&self) -> U256 {
        *self
    }
}

pub struct TxpoolService<DP: AccountInfoProvider, GP: GasPricer> {
    pool: Arc<AsyncMutex<Pool<DP>>>,
    gas_pricer: GP,
}

impl<DP: AccountInfoProvider, GP: GasPricer> TxpoolService<DP, GP> {
    pub fn new(pool: Arc<AsyncMutex<Pool<DP>>>, gas_pricer: GP) -> Self {
        Self { pool, gas_pricer }
    }
}

#[async_trait]
impl<DP: AccountInfoProvider, GP: GasPricer> Txpool for TxpoolService<DP, GP> {
    async fn find_unknown_transactions(
        &self,
        request: tonic::Request<TxHashes>,
    ) -> Result<tonic::Response<TxHashes>, tonic::Status> {
        let all_hashes = request.into_inner().hashes;

        let unknown_hashes = {
            let mut out = Vec::with_capacity(all_hashes.len());
            let pool = self.pool.lock().await;

            for hash in all_hashes {
                if hash.len() != H256::len_bytes() {
                    return Err(tonic::Status::invalid_argument("invalid hash"));
                }

                let hash = H256::from_slice(&hash);

                if pool.get(hash).is_none() {
                    out.push(hash.to_fixed_bytes().to_vec())
                }
            }

            out
        };

        Ok(tonic::Response::new(TxHashes {
            hashes: unknown_hashes,
        }))
    }

    async fn inject_transactions(
        &self,
        request: tonic::Request<InjectRequest>,
    ) -> Result<tonic::Response<InjectReply>, tonic::Status> {
        let txs = request.into_inner().txs;

        let mut out = Vec::with_capacity(txs.len());

        let minimum_gas_price = self.gas_pricer.minimum_gas_price();

        let txs = txs.into_iter().filter_map(|bytes| {
            if let Ok(tx) = rlp::decode::<Transaction>(&bytes) {
                if tx.gas_price >= minimum_gas_price {
                    return Some(tx);
                } else {
                    out.push(InjectResult::FeeTooLow);
                }
            } else {
                out.push(InjectResult::Invalid);
            }
            None
        });

        let results = self.pool.lock().await.import_many(txs).await;

        Ok(tonic::Response::new(InjectReply {
            injected: out
                .into_iter()
                .chain(results.into_iter().map(|(_, res)| match res {
                    Ok(imported) => {
                        if imported {
                            InjectResult::AlreadyExists
                        } else {
                            InjectResult::Success
                        }
                    }
                    Err(e) => match e {
                        ImportError::InvalidTransaction(_) => InjectResult::Invalid,
                        ImportError::NonceGap => InjectResult::InternalError,
                        ImportError::StaleTransaction => InjectResult::Stale,
                        ImportError::InvalidSender(_) => InjectResult::Invalid,
                        ImportError::FeeTooLow => InjectResult::FeeTooLow,
                        ImportError::InsufficientBalance => InjectResult::Invalid,
                        ImportError::NoCurrentBlock => InjectResult::InternalError,
                        ImportError::Other(_) => InjectResult::InternalError,
                    },
                }))
                .map(|v| v as i32)
                .collect(),
        }))
    }

    async fn get_transactions(
        &self,
        request: tonic::Request<GetTransactionsRequest>,
    ) -> Result<tonic::Response<GetTransactionsReply>, tonic::Status> {
        let pool = self.pool.clone();

        let pool = pool.lock().await;

        let txs = request
            .into_inner()
            .hashes
            .into_iter()
            .filter_map(|hash| {
                if hash.len() == H256::len_bytes() {
                    if let Some(tx) = pool.get(H256::from_slice(&hash)) {
                        return Some(rlp::encode(tx));
                    }
                }

                None
            })
            .collect();

        Ok(tonic::Response::new(GetTransactionsReply { txs }))
    }
}
