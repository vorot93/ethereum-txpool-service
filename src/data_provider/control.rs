use super::AccountInfoProvider;
use crate::grpc::txpool::{txpool_control_client::*, *};
use anyhow::anyhow;
use async_trait::async_trait;
use ethereum_txpool::AccountInfo;
use ethereum_types::{Address, H256};
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
                block_hash: Some(block.into()),
                account: Some(address.into()),
            })
            .await?
            .into_inner();

        Ok(Some(AccountInfo {
            balance: acc_info
                .balance
                .ok_or_else(|| anyhow!("no balance"))?
                .into(),
            nonce: acc_info.nonce,
        }))
    }
}
