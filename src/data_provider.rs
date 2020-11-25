use crate::grpc::txpool_control::{txpool_control_client::*, *};
use anyhow::anyhow;
use async_trait::async_trait;
use auto_impl::auto_impl;
use ethereum_txpool::AccountInfo;
use ethereum_types::{Address, H256, U256};
use std::{convert::TryInto, fmt::Display};
use tonic::transport::Channel;

#[async_trait]
#[auto_impl(&, Box, Arc)]
pub trait AccountInfoProvider: Send + Sync + 'static {
    async fn get_account_info(
        &self,
        block: H256,
        address: Address,
    ) -> anyhow::Result<Option<AccountInfo>>;
}

pub struct Web3DataProvider {
    client: web3::Web3<web3::transports::Http>,
}

impl Web3DataProvider {
    pub fn new<D: Display>(addr: D) -> anyhow::Result<Self> {
        let addr = addr.to_string();
        let transport = web3::transports::Http::new(&addr)?;
        let client = web3::Web3::new(transport);

        Ok(Self { client })
    }
}

#[async_trait]
impl AccountInfoProvider for Web3DataProvider {
    async fn get_account_info(
        &self,
        block: H256,
        address: Address,
    ) -> anyhow::Result<Option<AccountInfo>> {
        let block = self
            .client
            .eth()
            .block(block.into())
            .await?
            .ok_or_else(|| anyhow!("block does not exist"))?
            .number
            .ok_or_else(|| anyhow!("block not confirmed"))?
            .as_u64();

        let (balance, nonce) = futures::future::try_join(
            async move { self.client.eth().balance(address, Some(block.into())).await },
            async move {
                self.client
                    .eth()
                    .transaction_count(address, Some(block.into()))
                    .await
            },
        )
        .await?;
        Ok(Some(AccountInfo {
            balance,
            nonce: nonce.try_into().map_err(|_| anyhow!("nonce overflow"))?,
        }))
    }
}

pub struct GrpcDataProvider {
    client: TxpoolControlClient<Channel>,
}

impl GrpcDataProvider {
    pub async fn connect<D: Display>(addr: D) -> anyhow::Result<Self> {
        Ok(Self {
            client: TxpoolControlClient::connect(addr.to_string()).await?,
        })
    }
}

#[async_trait]
impl AccountInfoProvider for GrpcDataProvider {
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
