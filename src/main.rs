//! This module implements a registration script for a blockchain network.
//! It allows users to register hotkeys using provided coldkeys and other parameters.

use clap::Parser;
use log::{error, info};
use scale_value::{Composite, Value};
use serde::Deserialize;
use sp_core::H256;
use std::sync::Arc;
use std::time::{Duration, Instant};
use subxt::ext::sp_core::{sr25519, Pair};
use subxt::tx::DefaultPayload;
use subxt::{tx::PairSigner, OnlineClient, SubstrateConfig};
use tokio::sync::Mutex;

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

    let signer = PairSigner::new(coldkey.clone());

    let loops = Arc::new(Mutex::new(0u64));

    // Prepare transaction payload once for efficiency
    let call_data = Composite::named([
        ("netuid", params.netuid.into()),
        ("hotkey", hotkey.public().0.to_vec().into()),
    ]);
    let payload = DefaultPayload::new("SubtensorModule", "burned_register", call_data);

    // Main registration loop - attempt immediately without waiting for blocks
    loop {
        // Increment and log loop count
        {
            let mut loops_guard = loops.lock().await;
            *loops_guard += 1;
            if *loops_guard % 10 == 1 {
                // Log every 10th attempt to reduce overhead
                info!(
                    "{} | {} | Attempting registration",
                    *loops_guard,
                    get_formatted_date_now()
                );
            }
        }

        // Check recycle cost
        // let recycle_cost_start = Instant::now();
        // let recycle_cost = get_recycle_cost(&client, params.netuid).await?;
        // let recycle_cost_duration = recycle_cost_start.elapsed();
        // info!("⏱️ get_recycle_cost took {:?}", recycle_cost_duration);
        // info!("💸 Current recycle cost: {}", recycle_cost);

        // Skip if cost exceeds maximum allowed
        // if recycle_cost > params.max_cost {
        //     warn!(
        //         "💸 Recycle cost ({}) exceeds threshold ({}). Skipping registration attempt.",
        //         recycle_cost, params.max_cost
        //     );
        //     tokio::time::sleep(Duration::from_secs(1)).await;
        //     continue;
        // }

        // Sign and submit the transaction directly without spawn
        let sign_and_submit_start: Instant = Instant::now();
        let result = match client
            .tx()
            .sign_and_submit_then_watch_default(&payload, &signer)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                error!("Transaction submission failed: {:?}", e);
                // Minimal delay before retry
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
        };

        let sign_and_submit_duration = sign_and_submit_start.elapsed();
        if sign_and_submit_duration > Duration::from_millis(200) {
            info!("⏱️ sign_and_submit took {:?}", sign_and_submit_duration);
        }

        // Wait for transaction finalization
        let finalization_start = Instant::now();
        match result.wait_for_finalized_success().await {
            Ok(events) => {
                let finalization_duration = finalization_start.elapsed();
                let block_hash: H256 = events.extrinsic_hash();
                info!(
                    "🎯 Registration successful! Hash: {}, Finalization: {:?}",
                    block_hash, finalization_duration
                );
                break; // Exit the loop on successful registration
            }
            Err(e) => {
                error!("Registration failed: {:?}", e);
                // Minimal delay before retry
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }

    Ok(())
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
