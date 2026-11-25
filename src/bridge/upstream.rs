//! The upstream behind the stdio bridge, connected on the first tool call.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use anyhow::Result;
use rmcp::model::Content;
use tokio::sync::Mutex;

use crate::client::{McpClient, ToolFailure};
use crate::connection::ConnectionInfo;

/// An open session and the connection it was opened over.
pub struct Session {
    pub client: McpClient,
    pub conn: ConnectionInfo,
}

type ConnectFuture = Pin<Box<dyn Future<Output = Result<Session>> + Send>>;

/// Opens a session. Runs again for every call until it succeeds, so a Desktop
/// started later, or a key saved later, is picked up without restarting the
/// bridge.
pub type Connect = Box<dyn Fn() -> ConnectFuture + Send + Sync>;

pub struct Upstream {
    pub is_remote: bool,
    connect: Connect,
    session: Mutex<Option<Arc<Session>>>,
    /// The connecting application's name, unless `--client` was given.
    caller: OnceLock<String>,
}

impl Upstream {
    pub fn new(is_remote: bool, connect: Connect) -> Self {
        Self {
            is_remote,
            connect,
            session: Mutex::new(None),
            caller: OnceLock::new(),
        }
    }

    /// Name the application this bridge serves, on the current session or the
    /// one opened later. `--client` takes precedence.
    pub async fn name_caller(&self, name: &str) {
        let _ = self.caller.set(name.to_string());
        if let Some(session) = self.session.lock().await.as_deref() {
            self.apply_caller(session);
        }
    }

    /// Test-only: an upstream that is already connected.
    #[cfg(test)]
    pub(crate) fn connected(session: Session) -> Self {
        Self {
            is_remote: session.conn.is_remote,
            connect: Box::new(|| panic!("the session is already open")),
            session: Mutex::new(Some(Arc::new(session))),
            caller: OnceLock::new(),
        }
    }

    /// The connecting application's name, once it introduced itself.
    pub fn caller(&self) -> Option<&str> {
        self.caller.get().map(String::as_str)
    }

    fn apply_caller(&self, session: &Session) {
        if session.conn.client_name.is_none() {
            if let Some(name) = self.caller.get() {
                session.client.name_caller(name);
            }
        }
    }

    /// The open session, connecting first if there is none. Failing to connect
    /// is reported as a failed tool call, so the model reads what to do about
    /// it instead of a bare protocol error.
    async fn session(&self) -> Result<Arc<Session>> {
        let mut current = self.session.lock().await;
        if let Some(session) = current.as_ref() {
            return Ok(session.clone());
        }
        let session = (self.connect)().await.map_err(|err| ToolFailure {
            text: format!("{err:#}"),
        })?;
        self.apply_caller(&session);
        let session = Arc::new(session);
        *current = Some(session.clone());
        Ok(session)
    }

    pub async fn call_tool(&self, name: &str, args: serde_json::Value) -> Result<String> {
        let session = self.session().await?;
        session.client.call_tool(name, args, &session.conn).await
    }

    pub async fn call_tool_content(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<Vec<Content>> {
        let session = self.session().await?;
        session
            .client
            .call_tool_content(name, args, &session.conn)
            .await
    }
}
