use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::ToSocketAddrs;

use anyhow::Context;
use chrono::Local;
use ethers::contract::abigen;
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Provider};
use ethers::signers::Wallet;
use k256::ecdsa::SigningKey;
use serde::{Deserialize, Serialize};

pub type SignerClient = SignerMiddleware<Provider<Http>, Wallet<SigningKey>>;

abigen!(
    BillingContract,
    "src/contracts/billing_contract_abi.json",
    derives(serde::Serialize, serde::Deserialize)
);

#[derive(Debug, Serialize, Deserialize)]
pub struct InspectBody {
    pub bill: HashMap<String, u128>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportBody {
    pub bill_claim_data: String,
    pub signature: String,
}

pub fn is_valid_ip_with_port(ip_port_str: &str) -> bool {
    if let Ok(socket_addr) = ip_port_str.to_socket_addrs() {
        for addr in socket_addr {
            if addr.is_ipv4() {
                return true;
            }
        }
    }

    false
}

pub fn log_data(data: String) {
    if let Err(err) = OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open("logs.log")
        .and_then(|mut f| f.write_all(data.as_bytes()))
        .context("Error accessing/writing to the log file")
    {
        eprintln!("[{}] {}", Local::now().format("%Y-%m-%d %H:%M:%S"), err);
        println!("{}", data);
    }
}
