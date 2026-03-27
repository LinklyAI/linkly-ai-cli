mod bridge;
mod cli;
mod client;
mod commands;
mod connection;
mod constants;
mod output;
#[cfg(test)]
mod test_helpers;

use clap::Parser;
use cli::{AuthAction, Cli, Command, ConnectionArgs};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json_mode = cli.json;

    // Silent version check in background (non-blocking)
    let update_check = tokio::spawn(commands::self_update::check_silently());

    let result = run(cli).await;

    // Show update hint if available (only in non-JSON mode)
    if !json_mode {
        if let Ok(Some(new_version)) = update_check.await {
            eprintln!(
                "\nA new version is available: v{}. Run `linkly self-update` to upgrade.",
                new_version
            );
        }
    }

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}

fn resolve_conn(conn: &ConnectionArgs) -> anyhow::Result<connection::ConnectionInfo> {
    connection::resolve(conn.endpoint.as_deref(), conn.token.as_deref(), conn.remote)
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let json_mode = cli.json;

    match cli.command {
        Command::Auth { action } => match action {
            AuthAction::SetKey { key } => commands::auth::set_key(&key),
        },
        Command::Status { conn } => {
            let conn = resolve_conn(&conn)?;
            commands::status::run(&conn, json_mode).await
        }
        Command::SelfUpdate => commands::self_update::run().await,
        Command::Mcp { endpoint } => commands::mcp::run(endpoint.as_deref()).await,
        Command::ListLibraries { conn } => {
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::list_libraries::run(&client, json_mode).await
        }
        Command::Search {
            query,
            limit,
            r#type,
            library,
            path_glob,
            conn,
        } => {
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::search::run(&client, &query, limit, r#type, library, path_glob, json_mode)
                .await
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
            conn,
        } => {
            let conn = resolve_conn(&conn)?;
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
        Command::Outline { ids, conn } => {
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::outline::run(&client, &ids, json_mode).await
        }
        Command::Read {
            id,
            offset,
            limit,
            conn,
        } => {
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::read::run(&client, &id, offset, limit, json_mode).await
        }
    }
}
