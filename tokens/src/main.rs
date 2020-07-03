use solana_cli_config::{Config, CONFIG_FILE};
use solana_sdk::banks_client::start_tcp_client;
use solana_tokens::{arg_parser::parse_args, args::Command, commands, thin_client::ThinClient};
use std::{env, error::Error, path::Path, process};
use tokio::runtime::Runtime;

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
    let banks_client = runtime.block_on(start_tcp_client(&json_rpc_url))?;

    match command_args.command {
        Command::DistributeTokens(args) => {
            let mut thin_client = ThinClient::new(banks_client, args.dry_run);
            runtime.block_on(commands::process_distribute_tokens(&mut thin_client, &args))?;
        }
        Command::Balances(args) => {
            let mut thin_client = ThinClient::new(banks_client, false);
            runtime.block_on(commands::process_balances(&mut thin_client, &args))?;
        }
        Command::TransactionLog(args) => {
            commands::process_transaction_log(&args)?;
        }
    }
    Ok(())
}
