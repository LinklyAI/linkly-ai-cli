mod bridge;
mod cli;
mod client;
mod commands;
mod connection;
mod constants;
mod manifest;
mod outcome;
mod output;
#[cfg(test)]
mod test_helpers;
mod version_check;

use clap::Parser;
use cli::{AuthAction, Cli, Command, ConnectionArgs};
use outcome::Outcome;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json_mode = cli.json;

    // Silent version check in background (non-blocking)
    let update_check = tokio::spawn(commands::self_update::check_silently());

    // Write installed manifest off the async runtime — even though the
    // I/O is tiny, blocking the executor for synchronous filesystem
    // calls is the wrong shape for an async main and would stall any
    // concurrent background task (e.g. the update check above).
    let _ = tokio::task::spawn_blocking(manifest::write_manifest).await;

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

    match result {
        // 0 when the command produced results, 1 when it definitively found
        // nothing — see `outcome` for why "nothing" is only ever reported when
        // we can actually tell.
        Ok(outcome) => {
            let code = outcome.exit_code();
            if code != 0 {
                std::process::exit(code);
            }
        }
        Err(e) => {
            // `output::print_error` returns an empty-message Err after
            // already displaying the user-visible text; suppress our own
            // "Error: …" line in that case to avoid the duplicate. Any
            // other error reaches us with a real message and gets printed.
            let msg = format!("{:#}", e);
            if !msg.is_empty() {
                eprintln!("Error: {}", msg);
            }
            // 2 = the command failed, kept distinct from 1 = ran fine but
            // matched nothing.
            std::process::exit(2);
        }
    }
}

/// Commands with no notion of "found nothing" — reading a document, checking
/// status, saving a note — always exit 0 on success.
fn found(_: ()) -> Outcome {
    Outcome::Found
}

fn resolve_conn(conn: &ConnectionArgs) -> anyhow::Result<connection::ConnectionInfo> {
    connection::resolve(conn.endpoint.as_deref(), conn.token.as_deref(), conn.remote)
}

async fn run(cli: Cli) -> anyhow::Result<Outcome> {
    let json_mode = cli.json;

    match cli.command {
        Command::Auth { action } => match action {
            AuthAction::SetKey { key } => commands::auth::set_key(&key),
            AuthAction::Status => commands::auth::status(json_mode).await,
            AuthAction::Logout => commands::auth::logout(json_mode),
        }
        .map(found),
        Command::Status { conn } => {
            let conn = resolve_conn(&conn)?;
            commands::status::run(&conn, json_mode).await.map(found)
        }
        Command::Doctor { conn } => commands::doctor::run_from_args(&conn, json_mode)
            .await
            .map(found),
        Command::SelfUpdate => commands::self_update::run().await.map(found),
        Command::Mcp { endpoint, remote } => commands::mcp::run(endpoint.as_deref(), remote)
            .await
            .map(found),
        Command::ListLibraries { conn } => {
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::list_libraries::run(&client, &conn, json_mode)
                .await
                .map(found)
        }
        Command::Explore { library, conn } => {
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::explore::run(&client, &conn, library, json_mode)
                .await
                .map(found)
        }
        Command::FindPaths {
            patterns,
            library,
            limit,
            conn,
        } => {
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::find_paths::run(&client, &conn, patterns, library, limit, json_mode).await
        }
        Command::Search {
            query,
            limit,
            r#type,
            library,
            path_glob,
            modified_after,
            modified_before,
            time_sort,
            scope,
            tags,
            conn,
        } => {
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::search::run(
                &client,
                &conn,
                &query,
                limit,
                r#type,
                library,
                path_glob,
                modified_after,
                modified_before,
                time_sort,
                scope,
                tags,
                json_mode,
            )
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
                &conn,
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
        Command::Outline { ids, expand, conn } => {
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::outline::run(&client, &conn, &ids, expand, json_mode)
                .await
                .map(found)
        }
        Command::Read {
            id,
            offset,
            limit,
            image_text,
            conn,
        } => {
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::read::run(&client, &conn, &id, offset, limit, image_text, json_mode)
                .await
                .map(found)
        }
        Command::List {
            scope,
            tags,
            limit,
            offset,
            sort,
            no_snippet,
            conn,
        } => {
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::list::run(
                &client, &conn, &scope, tags, limit, offset, sort, no_snippet, json_mode,
            )
            .await
        }
        Command::NoteSave {
            mode,
            content,
            note_id,
            base_version,
            tags,
            conn,
        } => {
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::note_save::run(
                &client,
                &conn,
                &mode,
                &content,
                note_id,
                base_version,
                tags,
                json_mode,
            )
            .await
            .map(found)
        }
    }
}
