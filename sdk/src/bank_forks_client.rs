use solana_sdk::{
    clock::Slot,
    fee_calculator::FeeCalculator,
    hash::Hash,
    signature::Signature,
    transaction::{self, Transaction},
};

#[tarpc::service]
pub trait BankForksRpc {
    async fn get_recent_blockhash() -> (Hash, FeeCalculator, Slot);
    async fn send_transaction(transaction: Transaction) -> Signature;
    async fn get_signature_status(signature: Signature) -> Option<transaction::Result<()>>;
    async fn get_root_slot() -> Slot;
    async fn send_and_confirm_transaction(
        transaction: Transaction,
    ) -> Option<transaction::Result<()>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tarpc::{client, transport};

    #[test]
    fn test_bank_forks_rpc_client_new() {
        let (client_transport, _server_transport) = transport::channel::unbounded();
        BankForksRpcClient::new(client::Config::default(), client_transport);
    }
}
