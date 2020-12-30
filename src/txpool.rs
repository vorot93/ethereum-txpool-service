use crate::{
    data_provider::*,
    grpc::txpool::{self, txpool_server::Txpool, *},
};
use async_trait::async_trait;
use auto_impl::auto_impl;
use ethereum::Transaction;
use ethereum_txpool::{ImportError, Pool};
use ethereum_types::*;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex as AsyncMutex;
use tracing::*;
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
    pool: Arc<AsyncMutex<Pool>>,
    data_provider: DP,
    gas_pricer: GP,
}

impl<DP: AccountInfoProvider, GP: GasPricer> TxpoolService<DP, GP> {
    pub fn new(pool: Arc<AsyncMutex<Pool>>, data_provider: DP, gas_pricer: GP) -> Self {
        Self {
            pool,
            data_provider,
            gas_pricer,
        }
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

    async fn import_transactions(
        &self,
        request: tonic::Request<ImportRequest>,
    ) -> Result<tonic::Response<ImportReply>, tonic::Status> {
        let txs = request.into_inner().txs;

        let mut out = Vec::with_capacity(txs.len());

        let minimum_gas_price = self.gas_pricer.minimum_gas_price();

        let txs = txs
            .into_iter()
            .filter_map(|bytes| {
                if let Ok(tx) = rlp::decode::<Transaction>(&bytes) {
                    if tx.gas_price >= minimum_gas_price {
                        return Some(tx);
                    } else {
                        out.push(ImportResult::FeeTooLow);
                    }
                } else {
                    out.push(ImportResult::Invalid);
                }
                None
            })
            .collect::<Vec<_>>();

        let results = {
            let mut pool = self.pool.lock().await;

            // First pass: try to import with the state that's already there.
            let mut v = pool.import_many(txs.iter().cloned());

            if let Some(block) = pool.current_block() {
                // Assemble a full list of addresses for which we will request the state.
                let missing_state = v.iter().enumerate().fold(
                    HashMap::<Address, Vec<usize>>::new(),
                    |mut all, (idx, res)| {
                        if let Err(ImportError::NoState(address)) = res {
                            all.entry(*address).or_default().push(idx);
                        }

                        all
                    },
                );

                debug!(
                    "missing state for addresses: {:?}",
                    missing_state.keys().collect::<Vec<_>>()
                );

                // Fetch missing state.
                let additional_state = futures::future::join_all(missing_state.into_iter().map(
                    |(address, idxs)| async move {
                        (
                            self.data_provider
                                .get_account_info(block.hash, address)
                                .await,
                            address,
                            idxs,
                        )
                    },
                ))
                .await;

                let mut retry_tx_chains = Vec::with_capacity(additional_state.len());
                // Import missing state.
                for (res, address, idxs) in additional_state {
                    if let Ok(Some(info)) = res {
                        pool.add_account_state(address, info);
                        retry_tx_chains.push(idxs);
                    }
                }

                // Second pass: import the transactions again
                for chain in retry_tx_chains {
                    for (idx, res) in pool
                        .import_many(chain.iter().map(|&idx| txs[idx].clone()))
                        .into_iter()
                        .enumerate()
                    {
                        v[chain[idx]] = res;
                    }
                }
            }

            v
        };

        Ok(tonic::Response::new(ImportReply {
            imported: out
                .into_iter()
                .chain(results.into_iter().map(|res| match res {
                    Ok(imported) => {
                        if imported {
                            ImportResult::AlreadyExists
                        } else {
                            ImportResult::Success
                        }
                    }
                    Err(e) => match e {
                        ImportError::NoState(_) => ImportResult::InternalError,
                        ImportError::InvalidTransaction(_) => ImportResult::Invalid,
                        ImportError::NonceGap => ImportResult::InternalError,
                        ImportError::StaleTransaction => ImportResult::Stale,
                        ImportError::InvalidSender(_) => ImportResult::Invalid,
                        ImportError::FeeTooLow => ImportResult::FeeTooLow,
                        ImportError::InsufficientBalance => ImportResult::Invalid,
                        ImportError::NoCurrentBlock => ImportResult::InternalError,
                        ImportError::Other(_) => ImportResult::InternalError,
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
                        return Some(rlp::encode(tx).to_vec());
                    }
                }

                None
            })
            .collect();

        Ok(tonic::Response::new(GetTransactionsReply { txs }))
    }
}
