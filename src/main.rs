mod bridge;
mod cli;
mod client;
mod commands;
mod connection;
mod output;
#[cfg(test)]
mod test_helpers;

use clap::Parser;
use cli::{Cli, Command, AuthAction};

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
    // Extract shared fields before consuming cli.command
    let json_mode = cli.json;
    let endpoint = cli.endpoint;
    let token = cli.token;
    let remote = cli.remote;

    // Macro-like helper to reduce repetition
    macro_rules! conn {
        () => {
            connection::resolve(endpoint.as_deref(), token.as_deref(), remote)?
        };
    }

    match cli.command {
        Command::Auth { action } => match action {
            AuthAction::SetKey { key } => commands::auth::set_key(&key),
        },
        Command::Status => {
            let conn = conn!();
            commands::status::run(&conn, json_mode).await
        }
        Command::SelfUpdate => commands::self_update::run().await,
        Command::Mcp => {
            if remote {
                anyhow::bail!("`linkly mcp` does not support --remote.");
            }
            if token.is_some() {
                anyhow::bail!("`linkly mcp` does not support --token.");
            }
            commands::mcp::run(endpoint.as_deref()).await
        }
        Command::Search {
            query,
            limit,
            r#type,
        } => {
            let conn = conn!();
            let client = client::McpClient::connect(&conn).await?;
            commands::search::run(&client, &query, limit, r#type, json_mode).await
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
            let conn = conn!();
            let client = client::McpClient::connect(&conn).await?;
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
                json_mode,
            )
            .await
        }
        Command::Outline { ids } => {
            let conn = conn!();
            let client = client::McpClient::connect(&conn).await?;
            commands::outline::run(&client, &ids, json_mode).await
        }
        Command::Read { id, offset, limit } => {
            let conn = conn!();
            let client = client::McpClient::connect(&conn).await?;
            commands::read::run(&client, &id, offset, limit, json_mode).await
        }
    }
}
