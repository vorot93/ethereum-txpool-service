use super::AccountInfoProvider;
use crate::grpc::txpool_control::{txpool_control_client::*, *};
use async_trait::async_trait;
use ethereum_txpool::AccountInfo;
use ethereum_types::{Address, H256, U256};
use std::fmt::Display;
use tonic::transport::Channel;

pub struct ControlDataProvider {
    client: TxpoolControlClient<Channel>,
}

impl ControlDataProvider {
    pub async fn connect<D: Display>(addr: D) -> anyhow::Result<Self> {
        Ok(Self {
            client: TxpoolControlClient::connect(addr.to_string()).await?,
        })
    }
}

#[async_trait]
impl AccountInfoProvider for ControlDataProvider {
    async fn get_account_info(
        &self,
        block: H256,
        address: Address,
    ) -> anyhow::Result<Option<AccountInfo>> {
        let acc_info = self
            .client
            .clone()
            .account_info(AccountInfoRequest {
                block_hash: block.to_fixed_bytes().to_vec(),
                account: address.to_fixed_bytes().to_vec(),
            })
            .await?
            .into_inner();

        Ok(Some(AccountInfo {
            balance: U256::from_big_endian(&acc_info.balance),
            nonce: U256::from_big_endian(&acc_info.nonce).as_u64(),
        }))
    }
}
