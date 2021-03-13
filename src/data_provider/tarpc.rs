use super::AccountInfoProvider;
use async_trait::async_trait;
use ethereum_tarpc_api::*;
use std::fmt::Display;
use stubborn_io::{ReconnectOptions, StubbornTcpStream};
use tarpc::{client::Config, context, serde_transport::Transport};
use tokio_serde::formats::*;

#[derive(Debug)]
pub struct TarpcDataProvider {
    client: EthApiClient,
}

impl TarpcDataProvider {
    pub async fn new(addr: impl Display) -> anyhow::Result<Self> {
        let reconnect_opts = ReconnectOptions::new().with_exit_if_first_connect_fails(false);
        let tcp_stream =
            StubbornTcpStream::connect_with_options(addr.to_string(), reconnect_opts).await?;
        let transport = Transport::from((tcp_stream, Bincode::default()));
        Ok(Self {
            client: EthApiClient::new(Config::default(), transport).spawn()?,
        })
    }
}

#[async_trait]
impl AccountInfoProvider for TarpcDataProvider {
    async fn get_account_info(
        &self,
        block: ethereum_types::H256,
        address: ethereum_types::Address,
    ) -> anyhow::Result<Option<ethereum_txpool::AccountInfo>> {
        let (balance, nonce) = futures::future::try_join(
            async {
                self.client
                    .clone()
                    .balance(context::current(), block, address)
                    .await?
                    .map_err(anyhow::Error::msg)
            },
            async {
                self.client
                    .clone()
                    .nonce(context::current(), block, address)
                    .await?
                    .map_err(anyhow::Error::msg)
            },
        )
        .await?;

        if let Some(balance) = balance {
            if let Some(nonce) = nonce {
                return Ok(Some(ethereum_txpool::AccountInfo { balance, nonce }));
            }
        }

        Ok(None)
    }
}
