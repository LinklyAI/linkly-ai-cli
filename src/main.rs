mod bridge;
mod cli;
mod client;
mod commands;
mod connection;
mod output;

use clap::Parser;
use cli::{Cli, Command};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Silent version check in background (non-blocking)
    let update_check = tokio::spawn(commands::self_update::check_silently());

    let result = run(cli).await;

    // Show update hint if available (only in non-JSON mode)
    if let Ok(Some(new_version)) = update_check.await {
        eprintln!(
            "\nA new version is available: v{}. Run `linkly self-update` to upgrade.",
            new_version
        );
    }

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Status => {
            let conn = connection::resolve(cli.endpoint.as_deref())?;
            commands::status::run(&conn, cli.json).await
        }
        Command::SelfUpdate => commands::self_update::run().await,
        Command::Mcp => commands::mcp::run(cli.endpoint.as_deref()).await,
        Command::Search {
            query,
            limit,
            r#type,
        } => {
            let conn = connection::resolve(cli.endpoint.as_deref())?;
            let client = client::McpClient::connect(&conn.mcp_url).await?;
            commands::search::run(&client, &query, limit, r#type, cli.json).await
        }
        Command::Grep {
            pattern,
            doc_id,
            context,
            before,
            after,
            ignore_case,
            mode,
            limit,
            offset,
            fuzzy_whitespace,
        } => {
            let conn = connection::resolve(cli.endpoint.as_deref())?;
            let client = client::McpClient::connect(&conn.mcp_url).await?;
            commands::grep::run(
                &client,
                &pattern,
                &doc_id,
                context,
                before,
                after,
                ignore_case,
                mode,
                limit,
                offset,
                fuzzy_whitespace,
                cli.json,
            )
            .await
        }
        Command::Outline { ids } => {
            let conn = connection::resolve(cli.endpoint.as_deref())?;
            let client = client::McpClient::connect(&conn.mcp_url).await?;
            commands::outline::run(&client, &ids, cli.json).await
        }
        Command::Read { id, offset, limit } => {
            let conn = connection::resolve(cli.endpoint.as_deref())?;
            let client = client::McpClient::connect(&conn.mcp_url).await?;
            commands::read::run(&client, &id, offset, limit, cli.json).await
        }
    }
}
