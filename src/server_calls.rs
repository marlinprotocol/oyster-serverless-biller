use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use serde_json::json;

use crate::utils::{ExportBody, InspectBody};

pub async fn fetch_current_bill(billing_ip_port: &str) -> Result<HashMap<String, u128>> {
    let url = format!("http://{}/billing/inspect", billing_ip_port);
    let inspect_body = reqwest::get(url)
        .await
        .context(format!(
            "Failed to connect to the billing server at {}",
            billing_ip_port
        ))?
        .json::<InspectBody>()
        .await
        .context("Failed to parse the response into json body")?;

    Ok(inspect_body.bill)
}

pub async fn fetch_bill_receipt(
    billing_ip_port: &str,
    nonce: &String,
    exporting_tx_hashes: &Vec<String>,
) -> Result<Option<ExportBody>> {
    let url = format!("http://{}/billing/export", billing_ip_port);
    let signing_data = json!({
        "nonce": nonce,
        "tx_hashes": exporting_tx_hashes,
    });

    let response = reqwest::Client::new()
        .post(url)
        .json(&signing_data)
        .send()
        .await
        .context(format!(
            "Failed to connect to the billing server at {}",
            billing_ip_port
        ))?;

    if response.status().is_success() {
        return Ok(Some(
            response
                .json::<ExportBody>()
                .await
                .context("Failed to parse the response into json body")?,
        ));
    }

    if response.status().is_server_error() {
        eprintln!("[{}] Internal server error occurred while signing the bill receipt for nonce: {} and tx_hashes: {:?}", Local::now().format("%Y-%m-%d %H:%M:%S"), nonce, exporting_tx_hashes);
        return Ok(None);
    }

    Err(anyhow!(format!(
        "Error occurred while exporting the bill receipt: {}",
        response
            .text()
            .await
            .context("Failed to parse the response text")?
    )))
}

pub async fn fetch_last_bill_receipt(billing_ip_port: &str) -> Result<Option<ExportBody>> {
    let url = format!("http://{}/billing/latest", billing_ip_port);
    let response = reqwest::get(url).await.context(format!(
        "Failed to connect to the billing server at {}",
        billing_ip_port
    ))?;

    if response.status().is_success() {
        return Ok(Some(
            response
                .json::<ExportBody>()
                .await
                .context("Failed to parse the response into json body")?,
        ));
    }

    if response.status().is_client_error() {
        return Ok(None);
    }

    Err(anyhow!(format!(
        "Internal Server error occurred while exporting the bill receipt: {}",
        response
            .text()
            .await
            .context("Failed to parse the response text")?
    )))
}
