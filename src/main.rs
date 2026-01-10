//! This module implements a registration script for a blockchain network.
//! It allows users to register hotkeys using provided coldkeys and other parameters.

use clap::Parser;
use log::{error, info, warn};
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

    let signer = Arc::new(PairSigner::new(coldkey.clone()));

    let mut blocks = client.blocks().subscribe_finalized().await?;
    let loops = Arc::new(Mutex::new(0u64));

    // Cache the call_data for efficiency
    let call_data = Arc::new(Composite::named([
        ("netuid", params.netuid.into()),
        ("hotkey", hotkey.public().0.to_vec().into()),
    ]));

    // Prepare transaction payload
    let payload = Arc::new(DefaultPayload::new(
        "SubtensorModule",
        "burned_register",
        call_data.as_ref().clone(),
    ));

    // Main registration loop
    while let Some(block) = blocks.next().await {
        let block = block?;
        let block_number = block.header().number;

        // Increment and log loop count
        {
            let mut loops_guard = loops.lock().await;
            *loops_guard += 1;
            info!(
                "{} | {} | Attempting registration for block {}",
                *loops_guard,
                get_formatted_date_now(),
                block_number
            );
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

        // Sign and submit the transaction immediately for maximum speed
        // subxt will automatically fetch the latest nonce from the chain
        let sign_and_submit_start: Instant = Instant::now();
        let client_clone: Arc<OnlineClient<SubstrateConfig>> = Arc::clone(&client);
        let signer_clone: Arc<PairSigner<SubstrateConfig, sr25519::Pair>> = Arc::clone(&signer);
        let paylod_clone = Arc::clone(&payload);
        let result = match tokio::spawn(async move {
            // sign_and_submit_then_watch automatically queries the latest block state
            // to get the current nonce, preventing conflicts
            client_clone
                .tx()
                .sign_and_submit_then_watch(&*paylod_clone, &*signer_clone, Default::default())
                .await
        })
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                let error_str = format!("{:?}", e);
                // Check for nonce-related errors
                if error_str.contains("TooManyConsumers")
                    || error_str.contains("InvalidTransaction")
                    || error_str.contains("Stale")
                    || error_str.contains("nonce")
                {
                    warn!("Nonce-related error detected, will retry: {:?}", e);
                } else {
                    error!("Transaction submission failed: {:?}", e);
                }
                continue; // Continue to next iteration
            }
            Err(e) => {
                error!("Tokio spawn task failed: {:?}", e);
                continue; // Continue to next iteration
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
                    // Check if the error indicates the hotkey is already registered
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
                    {
                        warn!(
                            "[Block {}] Nonce-related error during finalization: {:?}",
                            block_num, e
                        );
                    } else {
                        error!("[Block {}] Registration failed: {:?}", block_num, e);
                    }
                }
            }
        });

        // Continue immediately to next block - no blocking wait
        // This ensures correct timing for each epoch opportunity
    }

    Ok(())
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
