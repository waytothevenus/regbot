//! This module implements a registration script for a blockchain network.
//! It allows users to register hotkeys using provided coldkeys and other parameters.

use clap::Parser;
use log::{error, info, warn};
use scale_value::{Composite, Value};
use serde::Deserialize;
use sp_core::H256;
use std::sync::Arc;
use std::time::{Duration, Instant};
use subxt::config::DefaultExtrinsicParamsBuilder;
use subxt::ext::sp_core::{sr25519, Pair};
use subxt::tx::DefaultPayload;
use subxt::{tx::PairSigner, OnlineClient, SubstrateConfig};

/// Struct to hold registration parameters, can be parsed from command line or config file
#[derive(Parser, Deserialize, Debug)]
#[clap(author, version, about, long_about = None)]
struct RegistrationParams {
    #[clap(long)]
    coldkey: String,

    #[clap(long)]
    hotkey: String,

    #[clap(long)]
    netuid: u16,

    #[clap(long, default_value = "5000000000")]
    max_cost: u64,

    #[clap(long, default_value = "wss://entrypoint-finney.opentensor.ai:443")]
    chain_endpoint: String,

    /// Slot number (0, 1, or 2) to determine which block within the 3-block registration window to target.
    /// - Slot 0: submits on blocks where block_number % 3 == 0
    /// - Slot 1: submits on blocks where block_number % 3 == 1  
    /// - Slot 2: submits on blocks where block_number % 3 == 2
    /// Run 3 instances with --slot 0, --slot 1, --slot 2 to register 3 miners per epoch.
    #[clap(long, default_value = "0")]
    slot: u32,
}

/// Returns the current date and time in Eastern Time Zone
///
/// # Returns
///
/// A `String` representing the current date and time in the format "YYYY-MM-DD HH:MM:SS TimeZone"
fn get_formatted_date_now() -> String {
    let now = chrono::Utc::now();
    let eastern_time = now.with_timezone(&chrono_tz::US::Eastern);
    eastern_time.format("%Y-%m-%d %H:%M:%S %Z%z").to_string()
}

/// Attempts to register a hotkey on the blockchain
///
/// # Arguments
///
/// * `params` - A reference to `RegistrationParams` containing registration details
///
/// # Returns
///
/// A `Result` which is `Ok` if registration is successful, or an `Err` containing the error message
// TODO: Parse event and decode Registered event
async fn register_hotkey(params: &RegistrationParams) -> Result<(), Box<dyn std::error::Error>> {
    // Initialize client connection to the blockchain
    let client = Arc::new(OnlineClient::<SubstrateConfig>::from_url(&params.chain_endpoint).await?);

    // Parse coldkey and hotkey from provided strings
    let coldkey: sr25519::Pair =
        sr25519::Pair::from_string(&params.coldkey, None).map_err(|_| "Invalid coldkey")?;
    let hotkey: sr25519::Pair =
        sr25519::Pair::from_string(&params.hotkey, None).map_err(|_| "Invalid hotkey")?;

    let signer = Arc::new(PairSigner::new(coldkey.clone()));

    // Track the last block we submitted on to avoid duplicate submissions
    let mut last_submitted_block: u32 = 0;
    let mut loop_count: u64 = 0;

    info!(
        "🚀 Starting registration bot for slot {} (will submit on blocks where block_number % 3 == {})",
        params.slot, params.slot
    );

    // Main registration loop - poll for latest block continuously
    // This approach is more reliable than subscriptions for time-sensitive operations
    loop {
        // Small delay to prevent overwhelming the RPC
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Fetch the absolute latest block
        let latest_block = match client.blocks().at_latest().await {
            Ok(b) => b,
            Err(e) => {
                warn!("Failed to fetch latest block: {:?}", e);
                continue;
            }
        };

        let block_number = latest_block.header().number;
        let block_hash = latest_block.hash();

        // Skip if we already submitted for this block
        if block_number <= last_submitted_block {
            continue;
        }

        // Check if this block matches our designated slot
        // Each instance targets blocks where block_number % 3 == slot
        let block_slot = block_number % 3;
        if block_slot != params.slot {
            // Update last seen block but don't submit
            last_submitted_block = block_number;
            info!(
                "⏭️ Skipping block {} (slot {}), waiting for slot {}",
                block_number, block_slot, params.slot
            );
            continue;
        }

        // This is our slot! Submit immediately
        last_submitted_block = block_number;
        loop_count += 1;

        info!(
            "{} | {} | 🎯 Slot {} - Attempting registration for block {} (hash: {})",
            loop_count,
            get_formatted_date_now(),
            params.slot,
            block_number,
            block_hash
        );

        // Prepare transaction payload fresh for each submission
        let call_data = Composite::named([
            ("netuid", params.netuid.into()),
            ("hotkey", hotkey.public().0.to_vec().into()),
        ]);

        let payload = DefaultPayload::new("SubtensorModule", "burned_register", call_data);

        // Sign and submit the transaction using the current latest block
        let sign_and_submit_start: Instant = Instant::now();

        // Use a long mortality period (256 blocks = ~51 minutes) to avoid outdated errors
        let tx_params = DefaultExtrinsicParamsBuilder::new()
            .mortal(latest_block.header(), 256)
            .build();

        let result = match client
            .tx()
            .sign_and_submit_then_watch(&payload, &*signer, tx_params)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                let error_str = format!("{:?}", e);
                // Check for recoverable errors
                if error_str.contains("TooManyConsumers")
                    || error_str.contains("InvalidTransaction")
                    || error_str.contains("Stale")
                    || error_str.contains("nonce")
                    || error_str.contains("outdated")
                    || error_str.contains("Transaction is outdated")
                {
                    warn!(
                        "Recoverable error detected, will retry on next matching slot: {:?}",
                        e
                    );
                } else {
                    error!("Transaction submission failed: {:?}", e);
                }
                continue;
            }
        };

        let sign_and_submit_duration = sign_and_submit_start.elapsed();
        info!("⏱️ sign_and_submit took {:?}", sign_and_submit_duration);

        // Spawn background task to monitor finalization (non-blocking)
        // This allows the main loop to continue immediately for correct timing
        let block_num = block_number;
        tokio::spawn(async move {
            let finalization_start = Instant::now();
            match result.wait_for_finalized_success().await {
                Ok(events) => {
                    let finalization_duration = finalization_start.elapsed();
                    info!(
                        "⏱️ [Block {}] wait_for_finalized_success took {:?}",
                        block_num, finalization_duration
                    );
                    let block_hash: H256 = events.extrinsic_hash();
                    info!(
                        "🎯 [Block {}] Registration successful! Extrinsic hash: {}",
                        block_num, block_hash
                    );
                    info!("✅ Registration completed! Bot continues attempting for next epoch opportunities...");
                }
                Err(e) => {
                    let error_str = format!("{:?}", e);
                    if error_str.contains("AlreadyRegistered")
                        || error_str.contains("already registered")
                        || error_str.contains("duplicate")
                    {
                        warn!(
                            "[Block {}] Hotkey appears to be already registered: {:?}",
                            block_num, e
                        );
                    } else if error_str.contains("TooManyConsumers")
                        || error_str.contains("InvalidTransaction")
                        || error_str.contains("Stale")
                        || error_str.contains("nonce")
                        || error_str.contains("outdated")
                    {
                        warn!(
                            "[Block {}] Recoverable error during finalization: {:?}",
                            block_num, e
                        );
                    } else {
                        error!("[Block {}] Registration failed: {:?}", block_num, e);
                    }
                }
            }
        });

        // Continue immediately to poll for next block
    }
}

/// Retrieves the current recycle cost for a given network UID
///
/// # Arguments
///
/// * `client` - A reference to the blockchain client
/// * `netuid` - The network UID to check
///
/// # Returns
///
/// A `Result` containing the recycle cost as a `u64` if successful, or an `Err` if retrieval fails
#[allow(dead_code)]
async fn get_recycle_cost(
    client: &OnlineClient<SubstrateConfig>,
    netuid: u16,
) -> Result<u64, Box<dyn std::error::Error>> {
    let latest_block = client.blocks().at_latest().await?;
    let burn_key = subxt::storage::dynamic(
        "SubtensorModule",
        "Burn",
        vec![Value::primitive(scale_value::Primitive::U128(
            netuid as u128,
        ))],
    );
    let burn_cost: u64 = client
        .storage()
        .at(latest_block.hash())
        .fetch(&burn_key)
        .await?
        .ok_or_else(|| "Burn value not found for the given netuid".to_string())?
        .as_type::<u64>()?;

    Ok(burn_cost)
}

// TODO: Return UID of the registered neuron
/// Main function to run the registration script
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging with INFO level
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    info!("Starting registration script...");

    // Parse configuration parameters
    let params: RegistrationParams = parse_config()?;

    // Attempt to register hotkey
    if let Err(e) = register_hotkey(&params).await {
        error!("Error during registration: {}", e);
        return Err(e);
    }

    info!("Registration process completed successfully.");
    Ok(())
}

/// Parses configuration from either a config file or command line arguments
///
/// # Returns
///
/// A `Result` containing `RegistrationParams` if parsing is successful, or an `Err` if it fails
fn parse_config() -> Result<RegistrationParams, Box<dyn std::error::Error>> {
    Ok(RegistrationParams::parse())
}
