use crate::{
    data_provider::DataProvider,
    grpc::txpool::{txpool_server::Txpool, *},
};
use async_trait::async_trait;
use auto_impl::auto_impl;
use ethereum::{Transaction, TransactionMessage};
use ethereum_types::*;
use parking_lot::Mutex;
use rlp::*;
use secp256k1::{
    recovery::{RecoverableSignature, RecoveryId},
    Message, SECP256K1,
};
use sha3::{Digest, Keccak256};
use std::{convert::TryFrom, sync::Arc};
use transaction_pool::*;

pub type Pool = transaction_pool::Pool<EthereumTransaction, EthereumScoring>;

#[derive(Clone, Debug)]
pub struct EthereumTransaction {
    inner: Transaction,
    sender: Address,
    hash: H256,
}

impl TryFrom<Transaction> for EthereumTransaction {
    type Error = anyhow::Error;

    fn try_from(tx: Transaction) -> Result<Self, Self::Error> {
        let h = {
            let mut stream = RlpStream::new();
            tx.rlp_append(&mut stream);
            Keccak256::digest(&stream.drain())
        };
        let hash = H256::from_slice(h.as_slice());

        let mut sig = [0_u8; 64];
        sig[..32].copy_from_slice(tx.signature.r().as_bytes());
        sig[32..].copy_from_slice(tx.signature.s().as_bytes());
        let rec = RecoveryId::from_i32(tx.signature.standard_v() as i32).unwrap();

        let public = &SECP256K1
            .recover(
                &Message::from_slice(TransactionMessage::from(tx.clone()).hash().as_bytes())?,
                &RecoverableSignature::from_compact(&sig, rec)?,
            )?
            .serialize_uncompressed()[1..];

        let sender = Address::from_slice(&Keccak256::digest(&public)[12..]);
        Ok(Self {
            sender,
            hash,
            inner: tx,
        })
    }
}

impl VerifiedTransaction for EthereumTransaction {
    type Hash = H256;
    type Sender = Address;

    fn hash(&self) -> &H256 {
        &self.hash
    }

    fn mem_usage(&self) -> usize {
        // TODO
        0
    }

    fn sender(&self) -> &Address {
        &self.sender
    }
}

impl EthereumTransaction {
    fn cost(&self) -> U256 {
        self.inner.gas_price * self.inner.gas_limit
    }
}

#[derive(Copy, Clone, Default, Debug)]
pub struct EthereumScoring;

impl Scoring<EthereumTransaction> for EthereumScoring {
    type Score = u64;
    type Event = ();

    fn compare(
        &self,
        old: &EthereumTransaction,
        other: &EthereumTransaction,
    ) -> std::cmp::Ordering {
        old.inner.nonce.cmp(&other.inner.nonce)
    }

    fn choose(&self, old: &EthereumTransaction, new: &EthereumTransaction) -> scoring::Choice {
        if old.inner.nonce != new.inner.nonce {
            return scoring::Choice::InsertNew;
        }

        if new.inner.gas_price > old.inner.gas_price {
            scoring::Choice::ReplaceOld
        } else {
            scoring::Choice::RejectNew
        }
    }

    fn update_scores(
        &self,
        _: &[transaction_pool::Transaction<EthereumTransaction>],
        _: &mut [Self::Score],
        _: scoring::Change<Self::Event>,
    ) {
    }
}

#[auto_impl(&, Box, Arc)]
pub trait GasPricer: Send + Sync + 'static {
    fn minimum_gas_price(&self) -> U256;
}

impl GasPricer for U256 {
    fn minimum_gas_price(&self) -> U256 {
        *self
    }
}

pub struct TxpoolService<DP: DataProvider, GP: GasPricer> {
    data_provider: DP,
    pool: Arc<Mutex<Pool>>,
    gas_pricer: GP,
}

impl<DP: DataProvider, GP: GasPricer> TxpoolService<DP, GP> {
    pub fn new(pool: Arc<Mutex<Pool>>, data_provider: DP, gas_pricer: GP) -> Self {
        Self {
            data_provider,
            pool,
            gas_pricer,
        }
    }
}

struct AlwaysReady;

impl<T> transaction_pool::Ready<T> for AlwaysReady {
    fn is_ready(&mut self, _: &T) -> Readiness {
        transaction_pool::Readiness::Ready
    }
}

#[async_trait]
impl<DP: DataProvider, GP: GasPricer> Txpool for TxpoolService<DP, GP> {
    async fn find_unknown_transactions(
        &self,
        request: tonic::Request<crate::grpc::txpool::TxHashes>,
    ) -> Result<tonic::Response<crate::grpc::txpool::TxHashes>, tonic::Status> {
        let all_hashes = request.into_inner().hashes;

        let unknown_hashes = {
            let mut out = Vec::with_capacity(all_hashes.len());
            let pool = self.pool.lock();

            for hash in all_hashes {
                if hash.len() != H256::len_bytes() {
                    return Err(tonic::Status::invalid_argument("invalid hash"));
                }

                let hash = H256::from_slice(&hash);

                if pool.find(&hash).is_none() {
                    out.push(hash.to_fixed_bytes().to_vec())
                }
            }

            out
        };

        Ok(tonic::Response::new(crate::grpc::txpool::TxHashes {
            hashes: unknown_hashes,
        }))
    }

    async fn inject_transaction(
        &self,
        request: tonic::Request<InjectRequest>,
    ) -> Result<tonic::Response<InjectReply>, tonic::Status> {
        let tx = EthereumTransaction::try_from(
            rlp::decode::<Transaction>(&request.into_inner().tx)
                .map_err(|e| tonic::Status::invalid_argument(&format!("invalid tx: {:?}", e)))?,
        )
        .map_err(|e| tonic::Status::invalid_argument(&format!("invalid tx: {:?}", e)))?;

        if tx.inner.nonce > u64::max_value().into() {
            return Err(tonic::Status::invalid_argument(
                "nonces greater than u64 are not supported",
            ));
        }

        let minimum_gas_price = self.gas_pricer.minimum_gas_price();

        Ok(tonic::Response::new(InjectReply {
            status: {
                if let Ok(None) = self.data_provider.confirmed_block(*tx.hash()).await {
                    let account_info = self
                        .data_provider
                        .account_info(*tx.sender())
                        .await
                        .map_err(|e| tonic::Status::internal(&format!("{}", e)))?;

                    if account_info.nonce > tx.inner.nonce.as_u64() {
                        InjectStatus::Stale
                    } else if tx.inner.gas_price < minimum_gas_price {
                        InjectStatus::FeeTooLow
                    } else {
                        let pool = self.pool.clone();
                        tokio::task::spawn_blocking(move || {
                            let mut pool = pool.lock();

                            struct ReplaceStrategy;

                            impl transaction_pool::ShouldReplace<EthereumTransaction> for ReplaceStrategy {
                                fn should_replace(
                                    &self,
                                    pooled_tx: &ReplaceTransaction<'_, EthereumTransaction>,
                                    new: &ReplaceTransaction<'_, EthereumTransaction>,
                                ) -> transaction_pool::scoring::Choice
                                {
                                    if pooled_tx.transaction.transaction.inner.nonce
                                        != new.transaction.transaction.inner.nonce
                                    {
                                        transaction_pool::scoring::Choice::InsertNew
                                    } else if new.transaction.transaction.inner.gas_price
                                        < pooled_tx.transaction.transaction.inner.gas_price
                                    {
                                        transaction_pool::scoring::Choice::RejectNew
                                    } else {
                                        transaction_pool::scoring::Choice::ReplaceOld
                                    }
                                }
                            }

                            let mut txs_to_remove = vec![];
                            let mut do_import = None;
                            let mut dropping = false;
                            let mut balance = account_info.balance;
                            let mut nonce = account_info.nonce;

                            // Cycle through existing transactions,
                            // - check if we need to replace in the middle.
                            // - drop txs with not enough balance if we replaced.
                            for pooled_tx in pool.pending_from_sender(AlwaysReady, &tx.sender()) {
                                assert_eq!(pooled_tx.inner.nonce.as_u64(), nonce);

                                if dropping {
                                    txs_to_remove.push(*pooled_tx.hash())
                                } else {
                                    // If this is a replacement transaction, validate and compare it against existing tx.
                                    if tx.inner.nonce == pooled_tx.inner.nonce {
                                        if tx.inner.gas_price >= pooled_tx.inner.gas_price {
                                            if let Some(new_balance) =
                                                balance.checked_sub(tx.cost())
                                            {
                                                balance = new_balance;
                                                txs_to_remove.push(*pooled_tx.hash());
                                                do_import = Some(true);
                                            } else {
                                                do_import = Some(false);
                                                break;
                                            }
                                        } else {
                                            do_import = Some(false);
                                            break;
                                        }
                                    } else {
                                        let new_balance = balance.checked_sub(pooled_tx.cost());

                                        if let Some(new_balance) = new_balance {
                                            balance = new_balance;
                                        } else {
                                            // Not enough balance for this transaction's gas.
                                            txs_to_remove.push(*pooled_tx.hash());
                                            // After dropping we will have a nonce gap. Drop other transactions.
                                            dropping = true;
                                        }
                                    }
                                }
                                nonce += 1;
                            }

                            // If this is not a replacement transaction, validate it as the next one.
                            if do_import.is_none()
                                && tx.inner.nonce.as_u64() == nonce
                                && balance.checked_sub(tx.cost()).is_some()
                            {
                                do_import = Some(true);
                            }

                            if let Some(true) = do_import {
                                for hash in txs_to_remove {
                                    pool.remove(&hash, false);
                                }

                                if pool.import(tx, &ReplaceStrategy).is_err() {
                                    InjectStatus::Invalid
                                } else {
                                    InjectStatus::Success
                                }
                            } else {
                                InjectStatus::Invalid
                            }
                        }).await.unwrap()
                    }
                } else {
                    InjectStatus::AlreadyExists
                }
            } as i32,
        }))
    }

    async fn get_transactions(
        &self,
        request: tonic::Request<GetTransactionsRequest>,
    ) -> Result<tonic::Response<GetTransactionsReply>, tonic::Status> {
        let txs = tokio::task::spawn_blocking({
            let pool = self.pool.clone();
            move || {
                let pool = pool.lock();

                request
                    .into_inner()
                    .hashes
                    .into_iter()
                    .filter_map(|hash| {
                        if hash.len() == H256::len_bytes() {
                            if let Some(tx) = pool.find(&H256::from_slice(&hash)) {
                                return Some(rlp::encode(&tx.inner));
                            }
                        }

                        None
                    })
                    .collect()
            }
        })
        .await
        .unwrap();

        Ok(tonic::Response::new(GetTransactionsReply { txs }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_literal::hex;

    #[test]
    fn test_parse_transaction() {
        let raw = hex!("f86e821d82850525dbd38082520894ce96e38eeff1c972f8f2851a7275b79b13b9a0498805e148839955f4008025a0ab1d3780cf338d1f86f42181ed13cd6c7fee7911a942c7583d36c9c83f5ec419a04984933928fac4b3242b9184aed633cc848f6a11d42af743f262ccf6592b8f71");

        let res = EthereumTransaction::try_from(rlp::decode::<Transaction>(&raw).unwrap()).unwrap();

        assert_eq!(
            &hex::encode(res.hash.0),
            "560ab39fe63d8fbbf688f0ebb6cfcacee4ce2ae8d85b096aa04c22a7c4c438af"
        );

        assert_eq!(
            &hex::encode(res.sender.as_bytes()),
            "3123c4396f1306678f382c186aee2dccce44f72c"
        );
    }
}
