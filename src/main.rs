mod bridge;
mod cli;
mod client;
mod commands;
mod connection;
mod constants;
mod doc_ids;
mod manifest;
mod outcome;
mod output;
mod skills;
#[cfg(test)]
mod test_helpers;
mod version_check;

use std::io::Write;

use clap::Parser;
use cli::{AuthAction, Cli, Command, ConnectionArgs};
use outcome::Outcome;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json_mode = cli.json;
    let exit_code_mode = cli.exit_code;
    let manages_skills = matches!(cli.command, Command::Skills { .. });
    // `linkly mcp` owns stdout: it is the JSON-RPC channel. A stray line there
    // breaks the client's parse before the handshake completes, so the bridge
    // delivers the notice as its own content block on the first tool result
    // instead (see bridge::handler::finish).
    let owns_stdout = matches!(cli.command, Command::Mcp { .. });

    // Silent version check in background (non-blocking)
    let update_check = tokio::spawn(commands::self_update::check_silently());
    // Same shape for the skill: bounded, silent on failure, and started early
    // so it overlaps the manifest write below rather than adding its latency
    // to the command.
    let skills_check = tokio::spawn(skills::check_silently());

    // Write installed manifest off the async runtime — even though the
    // I/O is tiny, blocking the executor for synchronous filesystem
    // calls is the wrong shape for an async main and would stall any
    // concurrent background task (e.g. the update check above).
    let _ = tokio::task::spawn_blocking(manifest::write_manifest).await;

    // The skills notice is resolved and printed BEFORE the command runs.
    //
    // Its audience is the agent driving the CLI, and a line appended after a
    // long search result is read as a footer — or dropped outright by clients
    // that truncate tool output. Ahead of the answer it is seen, and the wait
    // it costs is bounded: the check is throttled locally, so all but one run
    // every few hours resolves without touching the network.
    //
    // Publishing here also makes the JSON field exact. It used to be filled in
    // from whatever had landed by the time `run` printed, so a slow check
    // silently dropped the field from a machine-readable envelope.
    //
    // JSON mode gets the notice as a field on the envelope instead of a line,
    // so the output stays parseable. `linkly skills …` never carries it: the
    // state it describes is the one from before the command ran, and reporting
    // "not installed" right after an install succeeded is worse than silence.
    let skills_notice = skills_check.await.ok().flatten();
    skills::publish_hint(skills_notice.clone());
    if let Some(notice) = skills_notice {
        if notice_goes_to_stdout(json_mode, manages_skills, owns_stdout) {
            println!("{}", notice);
        }
    }

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

    // `std::process::exit` below skips destructors, and stdout is block-buffered
    // whenever it is not a terminal — which is every agent and every pipeline
    // reading us. Without this, an error path discards whatever is still
    // buffered: the skills notice, and the JSON error envelope that
    // `output::print_error` already wrote.
    let _ = std::io::stdout().flush();

    // Exit-code semantics are opt-in. Historically every successful run exited
    // 0 and every failure exited 1; making "found nothing" exit 1 by default
    // would silently repurpose that value and break existing scripts — the ones
    // testing `$? -eq 1` for failure, and the `set -e` ones that would now abort
    // on an empty search. `--exit-code` opts into the grep convention instead,
    // the same way `git diff --exit-code` does.
    match result {
        Ok(outcome) => {
            if exit_code_mode {
                let code = outcome.exit_code();
                if code != 0 {
                    std::process::exit(code);
                }
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
            // Under --exit-code, 2 keeps failures distinct from 1 = matched
            // nothing. Without it, 1 is the historical "something went wrong".
            std::process::exit(if exit_code_mode { 2 } else { 1 });
        }
    }
}

/// Whether the notice may be written to stdout as a plain line.
///
/// Each exclusion is a channel that already carries it, or cannot take it:
/// JSON mode puts it on the envelope, `linkly skills …` prints the real state
/// itself, and `linkly mcp` owns stdout for JSON-RPC — a line there breaks the
/// client's parse before the handshake finishes.
fn notice_goes_to_stdout(json_mode: bool, manages_skills: bool, owns_stdout: bool) -> bool {
    !json_mode && !manages_skills && !owns_stdout
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
        Command::Completions { shell } => commands::completions::run(shell).map(found),
        Command::SelfUpdate => commands::self_update::run().await.map(found),
        Command::Skills { action } => match action {
            cli::SkillsAction::Status => commands::skills::status(json_mode).await,
            cli::SkillsAction::Install => commands::skills::install().await,
            cli::SkillsAction::Update => commands::skills::update().await,
        }
        .map(found),
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
            doc_ids,
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
            let doc_ids = doc_ids::resolve(&doc_ids)?;
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::grep::run(
                &client,
                &conn,
                &pattern,
                &doc_ids,
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
            let ids = doc_ids::resolve(&ids)?;
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::outline::run(&client, &conn, &ids, expand, json_mode)
                .await
                .map(found)
        }
        Command::Read {
            ids,
            offset,
            limit,
            image_text,
            conn,
        } => {
            let ids = doc_ids::resolve(&ids)?;
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::read::run(&client, &conn, &ids, offset, limit, image_text, json_mode)
                .await
                .map(found)
        }
        Command::List {
            scope,
            library,
            path,
            r#type,
            modified_after,
            modified_before,
            tags,
            limit,
            offset,
            sort,
            snippet,
            no_snippet,
            conn,
        } => {
            let conn = resolve_conn(&conn)?;
            let client = client::McpClient::connect(&conn).await?;
            commands::list::run(
                &client,
                &conn,
                commands::list::ListParams {
                    scope,
                    library,
                    path,
                    doc_types: r#type,
                    modified_after,
                    modified_before,
                    tags,
                    limit,
                    offset,
                    sort,
                    snippet,
                    no_snippet,
                },
                json_mode,
            )
            .await
        }
        Command::NoteSave {
            mode,
            content,
            note_id,
            base_version,
            tags,
            app_name,
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
                app_name,
                json_mode,
            )
            .await
            .map(found)
        }
    }
}

#[cfg(test)]
mod notice_routing_tests {
    use super::notice_goes_to_stdout;

    #[test]
    fn plain_commands_print_the_notice() {
        assert!(notice_goes_to_stdout(false, false, false));
    }

    /// `linkly mcp` speaks JSON-RPC on stdout. A line there is not "a notice
    /// the client might ignore" — it is a parse error before the handshake
    /// completes, so the bridge sends it as a content block instead.
    #[test]
    fn the_mcp_bridge_never_prints_to_stdout() {
        assert!(!notice_goes_to_stdout(false, false, true));
    }

    #[test]
    fn json_and_skills_commands_carry_it_elsewhere() {
        assert!(!notice_goes_to_stdout(true, false, false));
        assert!(!notice_goes_to_stdout(false, true, false));
    }
}
