use async_trait::async_trait;
use auto_impl::auto_impl;
use ethereum_txpool::AccountInfo;
use ethereum_types::{Address, H256};

mod control;
mod tarpc;

pub use self::{control::*, tarpc::*};

#[async_trait]
#[auto_impl(&, Box, Arc)]
pub trait AccountInfoProvider: Send + Sync + 'static {
    async fn get_account_info(
        &self,
        block: H256,
        address: Address,
    ) -> anyhow::Result<Option<AccountInfo>>;
}
