use crate::api::ApiClient;
use crate::output::OutputFormat;
use anyhow::Result;
use clap::Subcommand;
use colored::Colorize;

#[derive(Subcommand)]
pub enum BoardCommands {
    /// Create a new board
    Create {
        /// Board name
        #[arg(short, long)]
        name: String,

        /// Your nickname in the board
        #[arg(short = 'N', long)]
        nickname: Option<String>,
    },
    /// List all boards
    List,
    /// Join a board
    Join {
        /// Board ID
        board_id: String,
    },
    /// Leave a board
    Leave {
        /// Board ID
        board_id: String,
    },
    /// Republish a board to the network
    ///
    /// Re-PUTs the board contract with its current state, making this node
    /// seed it again. Use when the board exists locally but isn't being
    /// served on the network.
    Republish {
        /// Board owner key (base58)
        board_id: String,
    },
    /// Update board configuration (owner only)
    Config {
        /// Board owner key (base58)
        board_id: String,

        /// Set maximum number of user bans remembered
        #[arg(long)]
        max_bans: Option<usize>,
    },
}

pub async fn execute(command: BoardCommands, api: ApiClient, format: OutputFormat) -> Result<()> {
    match command {
        BoardCommands::Create { name, nickname } => {
            // Ask for nickname if not provided
            let nickname = match nickname {
                Some(n) => n,
                None => {
                    if atty::is(atty::Stream::Stdin) {
                        dialoguer::Input::<String>::new()
                            .with_prompt("Enter your nickname")
                            .default("Anonymous".to_string())
                            .interact_text()?
                    } else {
                        "Anonymous".to_string()
                    }
                }
            };

            if !matches!(format, OutputFormat::Json) {
                eprintln!("Creating board '{}' with nickname '{}'...", name, nickname);
            }

            match api.create_board(name.clone(), nickname).await {
                Ok((owner_key, contract_key)) => {
                    let result = CreateBoardResult {
                        board_name: name,
                        owner_key: bs58::encode(owner_key.as_bytes()).into_string(),
                        contract_key: contract_key.id().to_string(),
                    };

                    match format {
                        OutputFormat::Human => {
                            println!("{}", "Board created successfully!".green());
                            println!("Owner key: {}", result.owner_key);
                            println!("Contract key: {}", result.contract_key);
                            println!("\nTo invite others, use:");
                            println!("  riverctl invite create {}", result.owner_key);
                        }
                        OutputFormat::Json => {
                            println!("{}", serde_json::to_string_pretty(&result)?);
                        }
                    }
                    Ok(())
                }
                Err(e) => {
                    eprintln!("{} {}", "Error:".red(), e);
                    Err(e)
                }
            }
        }
        BoardCommands::List => {
            if !matches!(format, OutputFormat::Json) {
                eprintln!("Listing boards...");
            }

            match api.list_boards().await {
                Ok(boards) => {
                    if boards.is_empty() {
                        match format {
                            OutputFormat::Human => {
                                println!("No boards found. Use 'riverctl board create' to create a new board.");
                            }
                            OutputFormat::Json => {
                                println!("[]");
                            }
                        }
                    } else {
                        match format {
                            OutputFormat::Human => {
                                println!("\n{} board(s) found:\n", boards.len());
                                for (owner_key, name, contract_key) in boards {
                                    println!("Board: {}", name.green());
                                    println!("  Owner key: {}", owner_key);
                                    println!("  Contract key: {}", contract_key);
                                    println!();
                                }
                            }
                            OutputFormat::Json => {
                                let json_boards: Vec<_> = boards
                                    .into_iter()
                                    .map(|(owner_key, name, contract_key)| {
                                        serde_json::json!({
                                            "name": name,
                                            "owner_key": owner_key,
                                            "contract_key": contract_key,
                                        })
                                    })
                                    .collect();
                                println!("{}", serde_json::to_string_pretty(&json_boards)?);
                            }
                        }
                    }
                    Ok(())
                }
                Err(e) => {
                    eprintln!("{} {}", "Error:".red(), e);
                    Err(e)
                }
            }
        }
        BoardCommands::Join { board_id } => {
            if !matches!(format, OutputFormat::Json) {
                eprintln!("Joining board: {}", board_id);
                eprintln!("To join a board, you need an invitation. Use 'riverctl invite accept <invitation-code>'");
            }
            Ok(())
        }
        BoardCommands::Leave { board_id } => {
            if !matches!(format, OutputFormat::Json) {
                eprintln!("Leaving board: {}", board_id);
            }
            // TODO: Implement board leaving
            Ok(())
        }
        BoardCommands::Config { board_id, max_bans } => {
            if max_bans.is_none() {
                // No changes requested, just show current config
                let owner_bytes = bs58::decode(&board_id)
                    .into_vec()
                    .map_err(|e| anyhow::anyhow!("Invalid board ID: {}", e))?;
                let owner_key = ed25519_dalek::VerifyingKey::from_bytes(
                    owner_bytes
                        .as_slice()
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("Invalid board ID length"))?,
                )
                .map_err(|e| anyhow::anyhow!("Invalid board owner key: {}", e))?;

                let board_state = api.get_board(&owner_key, false).await?;
                let cfg = &board_state.configuration.configuration;
                println!("Current configuration:");
                println!("  max_user_bans: {}", cfg.max_user_bans);
                return Ok(());
            }

            let owner_bytes = bs58::decode(&board_id)
                .into_vec()
                .map_err(|e| anyhow::anyhow!("Invalid board ID: {}", e))?;
            let owner_key = ed25519_dalek::VerifyingKey::from_bytes(
                owner_bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("Invalid board ID length"))?,
            )
            .map_err(|e| anyhow::anyhow!("Invalid board owner key: {}", e))?;

            if !matches!(format, OutputFormat::Json) {
                eprintln!("Updating board configuration...");
            }

            match api
                .update_config(&owner_key, |cfg| {
                    if let Some(v) = max_bans {
                        cfg.max_user_bans = v;
                    }
                })
                .await
            {
                Ok(()) => {
                    match format {
                        OutputFormat::Human => {
                            println!("{}", "Configuration updated successfully!".green());
                            if let Some(v) = max_bans {
                                println!("  max_user_bans: {}", v);
                            }
                        }
                        OutputFormat::Json => {
                            println!(
                                "{}",
                                serde_json::json!({
                                    "status": "success",
                                    "board_id": board_id,
                                })
                            );
                        }
                    }
                    Ok(())
                }
                Err(e) => {
                    eprintln!("{} {}", "Error:".red(), e);
                    Err(e)
                }
            }
        }
        BoardCommands::Republish { board_id } => {
            // Parse the board owner key
            let owner_bytes = bs58::decode(&board_id)
                .into_vec()
                .map_err(|e| anyhow::anyhow!("Invalid board ID: {}", e))?;
            let owner_key = ed25519_dalek::VerifyingKey::from_bytes(
                owner_bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("Invalid board ID length"))?,
            )
            .map_err(|e| anyhow::anyhow!("Invalid board owner key: {}", e))?;

            if !matches!(format, OutputFormat::Json) {
                eprintln!("Republishing board: {}", board_id);
            }

            match api.republish_board(&owner_key).await {
                Ok(()) => {
                    match format {
                        OutputFormat::Human => {
                            println!("{}", "Board republished successfully!".green());
                            println!("The board contract is now being seeded on the network.");
                        }
                        OutputFormat::Json => {
                            println!(
                                "{}",
                                serde_json::json!({
                                    "status": "success",
                                    "board_id": board_id,
                                })
                            );
                        }
                    }
                    Ok(())
                }
                Err(e) => {
                    eprintln!("{} {}", "Error:".red(), e);
                    Err(e)
                }
            }
        }
    }
}

#[derive(serde::Serialize)]
struct CreateBoardResult {
    board_name: String,
    owner_key: String,
    contract_key: String,
}
