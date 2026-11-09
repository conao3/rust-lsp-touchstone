//! The web UI of the strict client: a GraphQL API at `/api/graphql` (queries and mutations over
//! HTTP, subscriptions over WebSocket) and a single page at `/`.

use std::fs::File;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_graphql::{
    Context, Enum, Error, ID, InputObject, Object, Result, Schema, SimpleObject, Subscription,
};
use async_graphql_axum::{GraphQL, GraphQLSubscription};
use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, get_service};
use futures_util::{Stream, StreamExt};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::finding;
use crate::jsonrpc;
use crate::session::{self, Event, Session, SessionOptions};

const INDEX_HTML: &str = include_str!("../ui/index.html");

pub type ApiSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

pub struct AppState {
    session: Mutex<Option<Arc<Session>>>,
    next_session_id: Mutex<u64>,
    events: broadcast::Sender<Event>,
    record: Option<Arc<Mutex<File>>>,
    working_directory: PathBuf,
}

impl AppState {
    pub fn new(record: Option<File>) -> std::io::Result<Self> {
        let (events, _) = broadcast::channel(4096);
        Ok(AppState {
            session: Mutex::new(None),
            next_session_id: Mutex::new(1),
            events,
            record: record.map(|file| Arc::new(Mutex::new(file))),
            working_directory: std::env::current_dir()?,
        })
    }

    fn current(&self) -> Option<Arc<Session>> {
        self.session.lock().unwrap().clone()
    }

    fn running(&self) -> Result<Arc<Session>> {
        self.current()
            .ok_or_else(|| Error::new("no language server has been started"))
    }
}

pub fn schema(state: Arc<AppState>) -> ApiSchema {
    Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
        .data(state)
        .finish()
}

pub fn schema_sdl() -> String {
    Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
        .finish()
        .sdl()
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub enum Direction {
    ClientToServer,
    ServerToClient,
    ServerStderr,
}

impl From<session::Direction> for Direction {
    fn from(direction: session::Direction) -> Self {
        match direction {
            session::Direction::ClientToServer => Direction::ClientToServer,
            session::Direction::ServerToClient => Direction::ServerToClient,
            session::Direction::ServerStderr => Direction::ServerStderr,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub enum MessageKind {
    Request,
    Response,
    Notification,
    Invalid,
}

impl From<jsonrpc::Kind> for MessageKind {
    fn from(kind: jsonrpc::Kind) -> Self {
        match kind {
            jsonrpc::Kind::Request => MessageKind::Request,
            jsonrpc::Kind::Response => MessageKind::Response,
            jsonrpc::Kind::Notification => MessageKind::Notification,
            jsonrpc::Kind::Invalid => MessageKind::Invalid,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub enum SessionState {
    Running,
    Exited,
}

#[derive(SimpleObject)]
pub struct Finding {
    severity: Severity,
    code: String,
    message: String,
    offset: u64,
}

impl From<&finding::Finding> for Finding {
    fn from(f: &finding::Finding) -> Self {
        Finding {
            severity: match f.severity {
                finding::Severity::Error => Severity::Error,
                finding::Severity::Warning => Severity::Warning,
            },
            code: f.code.to_string(),
            message: f.message.clone(),
            offset: f.offset,
        }
    }
}

#[derive(SimpleObject)]
pub struct Message {
    session_id: ID,
    seq: u64,
    direction: Direction,
    time_ms: f64,
    kind: Option<MessageKind>,
    method: Option<String>,
    /// The JSON-RPC id as JSON text (`1`, `"a"`).
    id: Option<String>,
    /// The content of the frame as received or sent.
    body: String,
    findings: Vec<Finding>,
    /// The request this message answers.
    paired_seq: Option<u64>,
    /// Whether lsp-touchstone sent this message on its own (an answer to a server request).
    auto: bool,
}

impl From<&session::LoggedMessage> for Message {
    fn from(m: &session::LoggedMessage) -> Self {
        Message {
            session_id: ID(m.session_id.to_string()),
            seq: m.seq,
            direction: m.direction.into(),
            time_ms: m.time_ms,
            kind: m.kind.map(Into::into),
            method: m.method.clone(),
            id: m.id.clone(),
            body: m.body.clone(),
            findings: m.findings.iter().map(Into::into).collect(),
            paired_seq: m.paired_seq,
            auto: m.auto,
        }
    }
}

#[derive(SimpleObject)]
pub struct SessionInfo {
    id: ID,
    command: Vec<String>,
    state: SessionState,
    exit_code: Option<i32>,
    auto_respond: bool,
}

impl From<session::SessionInfo> for SessionInfo {
    fn from(info: session::SessionInfo) -> Self {
        SessionInfo {
            id: ID(info.id.to_string()),
            command: info.command,
            state: match info.state {
                session::SessionState::Running => SessionState::Running,
                session::SessionState::Exited => SessionState::Exited,
            },
            exit_code: info.exit_code,
            auto_respond: info.auto_respond,
        }
    }
}

#[derive(SimpleObject)]
pub struct Environment {
    /// The directory lsp-touchstone runs in, as an absolute path.
    working_directory: String,
    /// The same directory as a `file://` URI.
    working_directory_uri: String,
}

#[derive(InputObject)]
pub struct StartServerInput {
    /// The program and its arguments.
    command: Vec<String>,
    /// Answer requests from the server (`workspace/configuration` and others) automatically.
    #[graphql(default = true)]
    auto_respond: bool,
}

pub struct QueryRoot;

#[Object]
impl QueryRoot {
    async fn environment(&self, ctx: &Context<'_>) -> Result<Environment> {
        let state = ctx.data::<Arc<AppState>>()?;
        let dir = &state.working_directory;
        let uri = url::Url::from_directory_path(dir)
            .map_err(|()| Error::new("the working directory is not an absolute path"))?;
        Ok(Environment {
            working_directory: dir.display().to_string(),
            working_directory_uri: uri.as_str().trim_end_matches('/').to_string(),
        })
    }

    /// The current session, if a language server has been started.
    async fn session(&self, ctx: &Context<'_>) -> Result<Option<SessionInfo>> {
        let state = ctx.data::<Arc<AppState>>()?;
        Ok(state.current().map(|s| s.info().into()))
    }

    /// The messages of the current session, after `afterSeq` when it is given.
    async fn messages(&self, ctx: &Context<'_>, after_seq: Option<u64>) -> Result<Vec<Message>> {
        let state = ctx.data::<Arc<AppState>>()?;
        Ok(state
            .current()
            .map(|s| s.messages(after_seq).iter().map(Into::into).collect())
            .unwrap_or_default())
    }
}

fn parse_params(params: Option<String>) -> Result<Option<Value>> {
    match params {
        None => Ok(None),
        Some(text) if text.trim().is_empty() => Ok(None),
        Some(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|err| Error::new(format!("params is not JSON: {err}"))),
    }
}

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    /// Starts a language server, stopping the current one.
    async fn start_server(
        &self,
        ctx: &Context<'_>,
        input: StartServerInput,
    ) -> Result<SessionInfo> {
        let state = ctx.data::<Arc<AppState>>()?;
        if let Some(old) = state.current() {
            old.kill()?;
        }
        let id = {
            let mut next = state.next_session_id.lock().unwrap();
            let id = *next;
            *next += 1;
            id
        };
        let session = Session::start(
            id,
            SessionOptions {
                command: input.command,
                auto_respond: input.auto_respond,
            },
            state.events.clone(),
            state.record.clone(),
        )?;
        let info = session.info();
        *state.session.lock().unwrap() = Some(session);
        Ok(info.into())
    }

    /// Kills the language server process.
    async fn stop_server(&self, ctx: &Context<'_>) -> Result<SessionInfo> {
        let session = ctx.data::<Arc<AppState>>()?.running()?;
        session.kill()?;
        Ok(session.info().into())
    }

    /// Sends a request with the next id. `params` is JSON text.
    async fn send_request(
        &self,
        ctx: &Context<'_>,
        method: String,
        params: Option<String>,
    ) -> Result<Message> {
        let session = ctx.data::<Arc<AppState>>()?.running()?;
        Ok((&session.send_request(&method, parse_params(params)?)?).into())
    }

    /// Sends a notification. `params` is JSON text.
    async fn send_notification(
        &self,
        ctx: &Context<'_>,
        method: String,
        params: Option<String>,
    ) -> Result<Message> {
        let session = ctx.data::<Arc<AppState>>()?.running()?;
        Ok((&session.send_notification(&method, parse_params(params)?)?).into())
    }

    /// Sends `body` as the content of one frame without changing or validating it.
    async fn send_raw(&self, ctx: &Context<'_>, body: String) -> Result<Message> {
        let session = ctx.data::<Arc<AppState>>()?.running()?;
        Ok((&session.send_raw(body)?).into())
    }

    /// Sends `textDocument/didOpen` with the contents of a file. `path` is relative to the
    /// working directory unless it is absolute.
    async fn open_file(
        &self,
        ctx: &Context<'_>,
        path: String,
        language_id: String,
    ) -> Result<Message> {
        let state = ctx.data::<Arc<AppState>>()?;
        let session = state.running()?;
        let path = state.working_directory.join(path);
        Ok((&session.open_file(&path, &language_id)?).into())
    }
}

pub struct SubscriptionRoot;

#[Subscription]
impl SubscriptionRoot {
    /// Every message of every session as it is sent or received.
    async fn messages(&self, ctx: &Context<'_>) -> Result<impl Stream<Item = Message>> {
        let state = ctx.data::<Arc<AppState>>()?;
        Ok(
            BroadcastStream::new(state.events.subscribe()).filter_map(|event| async move {
                match event {
                    Ok(Event::Message(m)) => Some(Message::from(m.as_ref())),
                    _ => None,
                }
            }),
        )
    }

    /// The session's state whenever a server starts or exits.
    async fn session_changed(&self, ctx: &Context<'_>) -> Result<impl Stream<Item = SessionInfo>> {
        let state = ctx.data::<Arc<AppState>>()?;
        Ok(
            BroadcastStream::new(state.events.subscribe()).filter_map(|event| async move {
                match event {
                    Ok(Event::Session(info)) => Some(SessionInfo::from(info)),
                    _ => None,
                }
            }),
        )
    }
}

pub fn router(schema: ApiSchema, addr: SocketAddr) -> Router {
    let allowed = Arc::new(allowed_hosts(addr));
    Router::new()
        .route("/", get(|| async { Html(INDEX_HTML) }))
        .route(
            "/api/graphql",
            get_service(GraphQLSubscription::new(schema.clone()))
                .post_service(GraphQL::new(schema)),
        )
        .layer(middleware::from_fn(move |request, next| {
            let allowed = Arc::clone(&allowed);
            async move { check_origin(&allowed, request, next).await }
        }))
}

fn allowed_hosts(addr: SocketAddr) -> Vec<String> {
    let port = addr.port();
    vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")]
}

/// Accepts only requests addressed to this server from its own page, so that other web pages
/// open in the browser cannot start processes through the API.
async fn check_origin(allowed: &[String], request: Request<Body>, next: Next) -> Response {
    if !host_allowed(allowed, request.headers()) {
        return (StatusCode::FORBIDDEN, "unexpected Host or Origin").into_response();
    }
    next.run(request).await
}

fn host_allowed(allowed: &[String], headers: &HeaderMap) -> bool {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !allowed.iter().any(|a| a == host) {
        return false;
    }
    match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        None => true,
        Some(origin) => allowed.iter().any(|a| origin == format!("http://{a}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn schema_sdl_is_committed() {
        let committed = std::fs::read_to_string("schema.graphql").unwrap();
        assert_eq!(
            committed,
            schema_sdl(),
            "schema.graphql is stale; run `cargo run -- client print-graphql-schema > schema.graphql`"
        );
    }

    #[test]
    fn origin_and_host_are_checked() {
        let allowed = allowed_hosts("127.0.0.1:4000".parse().unwrap());
        let headers = |host: &str, origin: Option<&str>| {
            let mut map = HeaderMap::new();
            map.insert(header::HOST, HeaderValue::from_str(host).unwrap());
            if let Some(origin) = origin {
                map.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
            }
            map
        };
        assert!(host_allowed(&allowed, &headers("127.0.0.1:4000", None)));
        assert!(host_allowed(
            &allowed,
            &headers("localhost:4000", Some("http://localhost:4000"))
        ));
        assert!(!host_allowed(
            &allowed,
            &headers("127.0.0.1:4000", Some("https://example.com"))
        ));
        assert!(!host_allowed(&allowed, &headers("evil.example:4000", None)));
    }
}
