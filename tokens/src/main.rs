use solana_cli_config::Config;
use solana_cli_config::CONFIG_FILE;
use solana_sdk::banks_client::BanksClient;
use solana_tokens::{arg_parser::parse_args, args::Command, commands, thin_client::ThinClient};
use std::env;
use std::error::Error;
use std::path::Path;
use std::process;
use tarpc::client;
use tokio::runtime::Runtime;
use tokio_serde::formats::Json;

async fn start_client(json_rpc_url: &str) -> std::io::Result<BanksClient> {
    let transport = tarpc::serde_transport::tcp::connect(json_rpc_url, Json::default()).await?;
    BanksClient::new(client::Config::default(), transport).spawn()
}

fn main() -> Result<(), Box<dyn Error>> {
    let command_args = parse_args(env::args_os())?;
    let config = if Path::new(&command_args.config_file).exists() {
        Config::load(&command_args.config_file)?
    } else {
        let default_config_file = CONFIG_FILE.as_ref().unwrap();
        if command_args.config_file != *default_config_file {
            eprintln!("Error: config file not found");
            process::exit(1);
        }
        Config::default()
    };
    let json_rpc_url = command_args.url.unwrap_or(config.json_rpc_url);

    let mut runtime = Runtime::new().unwrap();
    let banks_client = runtime.block_on(start_client(&json_rpc_url))?;

    match command_args.command {
        Command::DistributeTokens(args) => {
            let mut thin_client = ThinClient::new(runtime, banks_client, args.dry_run);
            commands::process_distribute_tokens(&mut thin_client, &args)?;
        }
        Command::Balances(args) => {
            let mut thin_client = ThinClient::new(runtime, banks_client, false);
            commands::process_balances(&mut thin_client, &args)?;
        }
        Command::TransactionLog(args) => {
            commands::process_transaction_log(&args)?;
        }
    }
    Ok(())
}
