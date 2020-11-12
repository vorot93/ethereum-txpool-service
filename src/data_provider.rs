use anyhow::anyhow;
use async_trait::async_trait;
use auto_impl::auto_impl;
use ethereum_types::{Address, H256, U256};
use std::{convert::TryInto, fmt::Display};
use web3::types::TransactionId;

#[derive(Debug)]
pub struct AccountInfo {
    pub balance: U256,
    pub nonce: u64,
}

#[async_trait]
#[auto_impl(&, Box, Arc)]
pub trait DataProvider: Send + Sync + 'static {
    async fn confirmed_block(&self, txid: H256) -> anyhow::Result<Option<u64>>;
    async fn account_info(&self, address: Address) -> anyhow::Result<AccountInfo>;
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
impl DataProvider for Web3DataProvider {
    async fn confirmed_block(&self, txid: H256) -> anyhow::Result<Option<u64>> {
        Ok(self
            .client
            .eth()
            .transaction(TransactionId::Hash(txid))
            .await?
            .and_then(|tx| tx.block_number.map(|v| v.as_u64())))
    }

    async fn account_info(&self, address: Address) -> anyhow::Result<AccountInfo> {
        let block = self.client.eth().block_number().await?.as_u64();

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
        Ok(AccountInfo {
            balance,
            nonce: nonce.try_into().map_err(|_| anyhow!("nonce overflow"))?,
        })
    }
}
