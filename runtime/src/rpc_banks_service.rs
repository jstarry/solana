//! The `rpc_banks_service` module implements the Solana Banks RPC API.

use crate::{bank_forks::BankForks, banks_service::start_tcp_service};
use futures::{future::FutureExt, pin_mut, prelude::stream::StreamExt, select};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    thread::{self, Builder, JoinHandle},
};
use tokio::{
    runtime::Runtime,
    time::{self, Duration},
};

pub struct RpcBanksService {
    thread_hdl: JoinHandle<()>,
}

/// Run the TCP service until `exit` is set to true
async fn start_abortable_tcp_service(
    listen_addr: SocketAddr,
    tpu_addr: SocketAddr,
    bank_forks: Arc<RwLock<BankForks>>,
    exit: Arc<AtomicBool>,
) {
    let service = start_tcp_service(listen_addr, tpu_addr, bank_forks.clone()).fuse();
    let interval = time::interval(Duration::from_millis(100)).fuse();
    pin_mut!(service, interval);
    loop {
        select! {
            _ = service => {},
            _ = interval.select_next_some() => {
                if exit.load(Ordering::Relaxed) {
                    break;
                }
            }
        }
    }
}

impl RpcBanksService {
    fn run(
        listen_addr: SocketAddr,
        tpu_addr: SocketAddr,
        bank_forks: Arc<RwLock<BankForks>>,
        exit: Arc<AtomicBool>,
    ) {
        let service = start_abortable_tcp_service(listen_addr, tpu_addr, bank_forks, exit);
        Runtime::new().unwrap().block_on(service);
    }

    pub fn new(
        listen_addr: SocketAddr,
        tpu_addr: SocketAddr,
        bank_forks: &Arc<RwLock<BankForks>>,
        exit: &Arc<AtomicBool>,
    ) -> Self {
        let bank_forks = bank_forks.clone();
        let exit = exit.clone();
        let thread_hdl = Builder::new()
            .name("solana-rpc-banks".to_string())
            .spawn(move || Self::run(listen_addr, tpu_addr, bank_forks, exit))
            .unwrap();

        Self { thread_hdl }
    }

    pub fn join(self) -> thread::Result<()> {
        self.thread_hdl.join()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::Bank;

    #[test]
    fn test_rpc_banks_service_exit() {
        let bank_forks = Arc::new(RwLock::new(BankForks::new(Bank::default())));
        let exit = Arc::new(AtomicBool::new(false));
        let addr = "127.0.0.1:0".parse().unwrap();
        let service = RpcBanksService::new(addr, addr, &bank_forks, &exit);
        exit.store(true, Ordering::Relaxed);
        service.join().unwrap();
    }
}
