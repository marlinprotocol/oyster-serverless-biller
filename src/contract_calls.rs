use std::str::FromStr;

use anyhow::{Context, Result};
use chrono::Local;
use ethers::types::{TransactionReceipt, H256};
use ethers::{providers::Middleware, types::Bytes};

use crate::utils::{log_data, BillingContract, ExportBody, SignerClient};

pub async fn is_confirmation_receipt_pending(
    signer_client: &SignerClient,
    bill_tx_hash: H256,
) -> bool {
    let pending_receipt = signer_client
        .get_transaction_receipt(bill_tx_hash)
        .await
        .context(format!(
            "Error pulling confirmation receipt for the billing transaction {}",
            bill_tx_hash
        ));
    if let Err(err) = pending_receipt {
        log_data(format!(
            "[{}] {}",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            err
        ));
        return true;
    }

    let Ok(Some(receipt)) = pending_receipt else {
        return true;
    };

    log_data(format!(
        "[{}] Received confirmation receipt for the billing transaction {}: {:?}",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
        bill_tx_hash,
        receipt
    ));
    false
}

pub async fn send_billing_transaction(
    billing_contract: &BillingContract<SignerClient>,
    bill_receipt: &ExportBody,
) -> Result<(H256, Option<TransactionReceipt>)> {
    let txn = billing_contract.settle(
        Bytes::from_str(bill_receipt.bill_claim_data.as_str()).context(format!(
            "Failed to parse the bill data {} into ethers Bytes",
            bill_receipt.bill_claim_data
        ))?,
        Bytes::from_str(bill_receipt.signature.as_str()).context(format!(
            "Failed to parse the bill signature {} into ethers Bytes",
            bill_receipt.signature
        ))?,
    ); // parsing errors very unlikely

    let pending_txn = txn.send().await.context(format!(
        "Failed to send the billing transaction for receipt {:?}",
        bill_receipt
    ))?; // error if no signer available (not likely here) or rpc node doesn't have an unlocked account
    let bill_tx_hash = pending_txn.tx_hash();

    let Ok(bill_tx_receipt) = pending_txn.confirmations(3).await else {
        // TODO: FIX CONFIRMATIONS
        // rpc provider errors
        return Ok((bill_tx_hash, None));
    };

    Ok((bill_tx_hash, bill_tx_receipt))
}
