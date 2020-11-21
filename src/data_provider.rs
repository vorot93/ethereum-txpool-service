use anyhow::anyhow;
use async_trait::async_trait;
use ethereum_txpool::AccountInfo;
use ethereum_types::{Address, H256};
use std::{convert::TryInto, fmt::Display};

#[async_trait]
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
