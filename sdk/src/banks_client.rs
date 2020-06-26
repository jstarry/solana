use crate::{
    clock::Slot,
    fee_calculator::FeeCalculator,
    hash::Hash,
    pubkey::Pubkey,
    signature::Signature,
    transaction::{self, Transaction},
};

#[tarpc::service]
pub trait Banks {
    async fn get_fees() -> (FeeCalculator, Hash, Slot);
    async fn send_transaction(transaction: Transaction);
    async fn get_signature_status(signature: Signature) -> Option<transaction::Result<()>>;
    async fn get_root_slot() -> Slot;
    async fn send_and_confirm_transaction(
        transaction: Transaction,
    ) -> Option<transaction::Result<()>>;
    async fn get_balance(pubkey: Pubkey) -> u64;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tarpc::{client, transport};

    #[test]
    fn test_banks_client_new() {
        let (client_transport, _server_transport) = transport::channel::unbounded();
        BanksClient::new(client::Config::default(), client_transport);
    }
}
