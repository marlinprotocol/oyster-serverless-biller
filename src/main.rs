mod contract_calls;
mod server_calls;
mod utils;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use clap::Parser;
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Provider, ProviderExt};
use ethers::signers::{Signer, Wallet};
use ethers::types::{Address, H256};
use tokio::fs;
use tokio::time::{interval, Duration};
use tokio_retry::strategy::{jitter, ExponentialBackoff};
use tokio_retry::Retry;

use contract_calls::{is_confirmation_receipt_pending, send_billing_transaction};
use server_calls::{fetch_bill_receipt, fetch_current_bill, fetch_last_bill_receipt};
use utils::{is_valid_ip_with_port, BillingContract, ExportBody, SignerClient};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct CliArgs {
    #[clap(long, value_parser)]
    id: u32,

    #[clap(
        long,
        value_parser,
        default_value = "https://sepolia-rollup.arbitrum.io/rpc"
    )]
    rpc_url: String,

    #[clap(long, value_parser)]
    billing_ip_port: String,

    #[clap(long, value_parser)]
    billing_contract_addr: String,

    #[clap(long, value_parser)]
    secret_key_file: String,

    #[clap(long, value_parser)]
    payee_wallet_address: String,

    #[clap(long, value_parser, default_value = "")] // TODO: DEFAULT VALUE
    method_call_cost: u128,

    #[clap(long, value_parser, default_value = "")] // TODO: DEFAULT VALUE
    balance_transfer_cost: u128,

    #[clap(long, value_parser, default_value = "")] // TODO: DEFAULT VALUE
    billing_interval_secs: u64,
}

async fn biller(
    is_last_bill_exported: bool,
    bill_receipt: Option<ExportBody>,
    billing_ip_port: &str,
    billing_contract: &BillingContract<SignerClient>,
    method_call_cost: u128,
    balance_transfer_cost: u128,
    nonce: &mut [u8],
    payee: Address,
) -> (bool, Option<ExportBody>, Option<H256>) {
    if is_last_bill_exported {
        let last_bill_receipt = match bill_receipt {
            Some(bill_receipt) => bill_receipt,
            None => {
                let bill_receipt = Retry::spawn(
                    ExponentialBackoff::from_millis(100).map(jitter).take(5), // TODO: SET RETRIES AND BASE MILLIS
                    || async { fetch_last_bill_receipt(billing_ip_port).await },
                )
                .await
                .context("Error fetching the last bill receipt to claim");

                let Ok(bill_receipt) = bill_receipt else {
                    eprintln!(
                        "[{}] {}",
                        Local::now().format("%Y-%m-%d %H:%M:%S"),
                        bill_receipt.unwrap_err()
                    );
                    return (true, None, None);
                };

                let Some(bill_receipt) = bill_receipt else {
                    eprintln!(
                        "[{}] FATAL ERROR: Lost exported bill info pending to claim!!!",
                        Local::now().format("%Y-%m-%d %H:%M:%S")
                    );
                    return (false, None, None);
                };

                bill_receipt
            }
        };

        let billing_tx = send_billing_transaction(billing_contract, &last_bill_receipt, payee)
            .await
            .context("Error sending the billing transaction to the network");

        let Ok((bill_tx_hash, bill_tx_receipt)) = billing_tx else {
            eprintln!(
                "[{}] {}",
                Local::now().format("%Y-%m-%d %H:%M:%S"),
                billing_tx.unwrap_err()
            );
            return (true, Some(last_bill_receipt), None);
        };

        let Some(bill_tx_receipt) = bill_tx_receipt else {
            println!(
                "[{}] Bill submitted {}, PENDING confirmation receipt!!!",
                Local::now().format("%Y-%m-%d %H:%M:%S"),
                bill_tx_hash
            );
            return (false, None, Some(bill_tx_hash));
        };

        println!(
            "[{}] Bill submitted {} successfully with confirmation receipt: {:?}",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            bill_tx_hash,
            bill_tx_receipt
        );
    }

    let current_bill = Retry::spawn(
        ExponentialBackoff::from_millis(100).map(jitter).take(5), // TODO: SET RETRIES AND BASE MILLIS
        || async { fetch_current_bill(billing_ip_port).await },
    )
    .await
    .context("Error fetching the current bill");

    let Ok(current_bill) = current_bill else {
        eprintln!(
            "[{}] {}",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            current_bill.unwrap_err()
        );
        return (false, None, None);
    };

    let mut exporting_tx_hashes = Vec::new();
    let mut margin = 0;
    for (tx_hash, amount) in current_bill {
        if amount > balance_transfer_cost {
            exporting_tx_hashes.push(tx_hash);
            margin += amount - balance_transfer_cost;
        }
    }

    if exporting_tx_hashes.is_empty() || margin <= method_call_cost {
        println!(
            "[{}] Bill isn't worth claiming!!!",
            Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        return (false, None, None);
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("Error generating current timestamp for nonce");

    let Ok(timestamp) = timestamp else {
        eprintln!(
            "[{}] {}",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            timestamp.unwrap_err()
        );
        return (false, None, None);
    };

    nonce[24..].copy_from_slice(&timestamp.as_secs().to_be_bytes());
    let current_nonce = hex::encode(nonce);

    let bill_receipt = Retry::spawn(
        ExponentialBackoff::from_millis(100).map(jitter).take(5), // TODO: SET RETRIES AND BASE MILLIS
        || async {
            fetch_bill_receipt(billing_ip_port, &current_nonce, &exporting_tx_hashes).await
        },
    )
    .await
    .context("Error exporting the bill receipt");

    let Ok(bill_receipt) = bill_receipt else {
        eprintln!(
            "[{}] {}",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            bill_receipt.unwrap_err()
        );
        return (false, None, None);
    };

    let Some(bill_receipt) = bill_receipt else {
        return (true, None, None);
    };

    let bill_tx = send_billing_transaction(billing_contract, &bill_receipt, payee)
        .await
        .context("Error sending the billing transaction to the network");

    let Ok((bill_tx_hash, bill_tx_receipt)) = bill_tx else {
        eprintln!(
            "[{}] {}",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            bill_tx.unwrap_err()
        );
        return (true, Some(bill_receipt), None);
    };

    let Some(bill_tx_receipt) = bill_tx_receipt else {
        println!(
            "[{}] Bill submitted {}, PENDING confirmation receipt!!!",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            bill_tx_hash
        );
        return (false, None, Some(bill_tx_hash));
    };

    println!(
        "[{}] Bill submitted {} successfully with confirmation receipt: {:?}",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
        bill_tx_hash,
        bill_tx_receipt
    );

    (false, None, None)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = CliArgs::parse();

    if !is_valid_ip_with_port(&cli.billing_ip_port) {
        return Err(anyhow!(
            "Invalid Billing IP address {}!",
            cli.billing_ip_port
        ));
    }

    let rpc_provider = Provider::<Http>::try_connect(&cli.rpc_url)
        .await
        .context(format!("Error connecting to the rpc {}", cli.rpc_url))?;
    let signer_wallet = Wallet::from_bytes(
        hex::decode(
            fs::read_to_string(&cli.secret_key_file)
                .await
                .context(format!(
                    "Error reading the secret key file at path {}",
                    cli.secret_key_file
                ))?,
        )
        .context("Error decoding the secret key")?
        .as_slice(),
    )
    .context("Invalid secret key provided")?;
    let wallet_address = signer_wallet.address();
    let payee_wallet_address = cli
        .payee_wallet_address
        .parse::<Address>()
        .context(format!(
            "Error parsing the payee wallet address {} to eth address H160",
            cli.payee_wallet_address
        ))?;

    let signer_client = SignerMiddleware::new(rpc_provider, signer_wallet);
    let billing_contract = BillingContract::new(
        cli.billing_contract_addr
            .parse::<Address>()
            .context(format!(
                "Error parsing the billing contract address {} to eth bytes",
                cli.billing_contract_addr
            ))?,
        Arc::new(signer_client.clone()),
    );

    let mut nonce = [0u8; 32];
    nonce[..20].copy_from_slice(wallet_address.as_bytes());
    nonce[20..24].copy_from_slice(&cli.id.to_be_bytes());

    let mut is_last_bill_exported = false;
    let mut bill_receipt: Option<ExportBody> = None;
    let mut pending_bill_tx_hashes: Vec<H256> = Vec::new();
    let mut interval = interval(Duration::from_secs(cli.billing_interval_secs));

    loop {
        interval.tick().await;

        let mut updated_pending_bills: Vec<H256> = Vec::new();
        for tx_hash in pending_bill_tx_hashes {
            if is_confirmation_receipt_pending(&signer_client, tx_hash).await {
                updated_pending_bills.push(tx_hash);
            }
        }
        pending_bill_tx_hashes = updated_pending_bills;

        let mut _bill_tx_hash = None;
        (is_last_bill_exported, bill_receipt, _bill_tx_hash) = biller(
            is_last_bill_exported,
            bill_receipt,
            &cli.billing_ip_port,
            &billing_contract,
            cli.method_call_cost,
            cli.balance_transfer_cost,
            nonce.as_mut_slice(),
            payee_wallet_address,
        )
        .await;

        if let Some(bill_tx_hash) = _bill_tx_hash {
            pending_bill_tx_hashes.push(bill_tx_hash);
        }
    }
}
