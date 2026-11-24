//! `marsad-agent ask-mcp --socket <daemon socket> --session <id>`: a one-tool MCP server,
//! `ask_user`, that Marsad injects into sessions of CLIs whose own question tool cannot reach
//! Marsad (their descriptor reports `questions: false`). The tool asks the user through the
//! daemon (`ask.user`): a question gate on the calling session, shown and answered like a
//! native question, whose answer is the tool's result.

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use marsad_proto::ask::{AskUserReply, AskUserRequest};
use marsad_proto::methods;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo};
use rmcp::schemars::{self, JsonSchema};
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use serde::Deserialize;
use tokio::time::Instant;
use wasit_control::{
    connector, local_endpoint, CallError, ClientOptions, ControlClient, Hello, RequestOpts,
    Supervised, SupervisedOptions,
};
use wasit_core::{McpServerConfig, McpTransport, QuestionOption, QuestionPrompt};

use crate::host::ask::ANSWER_DEADLINE;

/// Name of the MCP server as agents see it (the tool appears as `marsad_ask_user` in
/// OpenCode / Kilo).
pub const SERVER_NAME: &str = "marsad";

/// The link the `ask.user` calls go over: the supervised daemon link in production, a fake in
/// tests.
pub trait AskLink: Sync {
    /// Wait up to `max` for a live connection. Event-driven: returns as soon as one is up.
    fn connected(&self, max: Duration) -> impl Future<Output = Result<(), CallError>> + Send;
    /// One `ask.user` call over the live connection, bounded by `deadline`.
    fn send(
        &self,
        req: &AskUserRequest,
        deadline: Duration,
    ) -> impl Future<Output = Result<AskUserReply, CallError>> + Send;
}

impl AskLink for Supervised {
    async fn connected(&self, max: Duration) -> Result<(), CallError> {
        self.wait_connected(max).await.map(|_| ())
    }

    async fn send(
        &self,
        req: &AskUserRequest,
        deadline: Duration,
    ) -> Result<AskUserReply, CallError> {
        let client = self.current().ok_or(CallError::NotConnected)?;
        client
            .request_with(methods::ASK_USER, req, RequestOpts::new(deadline))
            .await
    }
}

/// Asks `req` until the user answers or `deadline` passes. A lost connection is waited out (for
/// the link to come back), not counted: each attempt re-sends the same request id, so the daemon
/// waits for the gate it already raised instead of asking twice. The error text is what the
/// agent is told when the deadline passes, naming what was last wrong.
pub async fn ask_until_answered<L: AskLink>(
    link: &L,
    req: &AskUserRequest,
    deadline: Duration,
) -> Result<AskUserReply, String> {
    const LOST: &str = "the connection to Marsad was lost and not restored in time";
    const SILENT: &str = "Marsad did not answer in time";
    let until = Instant::now() + deadline;
    let mut last = SILENT;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!("no answer within {deadline:?}; {last}"));
        }
        if let Err(e) = link.connected(left).await {
            // `connected` only fails at its own limit (the deadline) or when the link is stopped.
            if Instant::now() < until {
                return Err(format!("the link to Marsad stopped ({e})"));
            }
            last = LOST;
            continue;
        }
        let left = until.saturating_duration_since(Instant::now());
        match link.send(req, left).await {
            Ok(reply) => return Ok(reply),
            Err(CallError::ConnectionLost(_) | CallError::NotConnected) => last = LOST,
            Err(CallError::Timeout) => {
                return Err(format!("no answer within {deadline:?}; {SILENT}"))
            }
            Err(CallError::Rpc(e)) => return Err(e.message),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// The MCP server item for a session (`program` is the daemon binary).
pub fn mcp_server_config(program: &str, socket: &str, session: &str) -> McpServerConfig {
    McpServerConfig {
        name: SERVER_NAME.into(),
        transport: McpTransport::Stdio {
            command: program.into(),
            args: vec![
                "ask-mcp".into(),
                "--socket".into(),
                socket.into(),
                "--session".into(),
                session.into(),
            ],
            env: Default::default(),
        },
        // Asking the user is what the tool is for; a permission prompt first is redundant.
        trusted: true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum AskKind {
    /// Pick one of `options` (the default when options are given).
    #[default]
    Single,
    /// Pick any number of `options`.
    Multiple,
    /// A typed answer (the default without options).
    Text,
    /// Yes or no.
    YesNo,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AskOption {
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AskArgs {
    /// The question, complete and self-contained.
    pub question: String,
    /// A short title for it (a few words), e.g. "Database".
    #[serde(default)]
    pub header: Option<String>,
    /// single (one option), multiple (several options), text (typed answer) or yes_no.
    #[serde(default)]
    pub kind: Option<AskKind>,
    /// The choices for single / multiple.
    #[serde(default)]
    pub options: Vec<AskOption>,
    /// Also accept a typed answer besides the options.
    #[serde(default)]
    pub allow_other: Option<bool>,
}

impl AskArgs {
    /// The question gate's prompt.
    pub fn prompt(&self) -> Result<QuestionPrompt, String> {
        let kind = self.kind.unwrap_or(if self.options.is_empty() {
            AskKind::Text
        } else {
            AskKind::Single
        });
        let opt = |label: &str, description: &str| QuestionOption {
            label: label.into(),
            description: description.into(),
        };
        let options: Vec<QuestionOption> = match kind {
            AskKind::YesNo => vec![opt("Yes", ""), opt("No", "")],
            AskKind::Text => Vec::new(),
            AskKind::Single | AskKind::Multiple => {
                if self.options.is_empty() {
                    return Err("`options` are required for a single or multiple choice".into());
                }
                self.options
                    .iter()
                    .map(|o| opt(&o.label, o.description.as_deref().unwrap_or("")))
                    .collect()
            }
        };
        Ok(QuestionPrompt {
            id: "answer".into(),
            question: self.question.clone(),
            header: self.header.clone().unwrap_or_default(),
            multi_select: kind == AskKind::Multiple,
            allow_other: kind == AskKind::Text || self.allow_other.unwrap_or(false),
            options,
            secret: false,
        })
    }
}

/// The MCP handler.
#[derive(Clone)]
pub struct AskMcp {
    link: Supervised,
    session: String,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl AskMcp {
    #[tool(
        description = "Ask the user a question and wait for the answer. Use it whenever you need a decision, a preference or information only the user has, instead of asking in your reply. kind: single (pick one of options), multiple (pick several), text (typed answer) or yes_no. Returns the chosen option label(s) or the typed text."
    )]
    async fn ask_user(
        &self,
        Parameters(a): Parameters<AskArgs>,
    ) -> Result<CallToolResult, McpError> {
        let question = match a.prompt() {
            Ok(q) => q,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };
        let req = AskUserRequest {
            session: self.session.clone(),
            request_id: format!("ask-{:016x}", fastrand::u64(..)),
            question,
        };
        let reply = ask_until_answered(&self.link, &req, ANSWER_DEADLINE).await;
        Ok(match reply {
            Ok(AskUserReply {
                cancelled: Some(why),
                ..
            }) => CallToolResult::error(vec![Content::text(format!(
                "The user did not answer: {why}."
            ))]),
            Ok(r) if r.answers.is_empty() => {
                CallToolResult::success(vec![Content::text("The user answered with nothing.")])
            }
            Ok(r) => CallToolResult::success(vec![Content::text(format!(
                "The user answered: {}",
                r.answers.join(", ")
            ))]),
            Err(e) => {
                CallToolResult::error(vec![Content::text(format!("Could not ask the user: {e}"))])
            }
        })
    }
}

#[tool_handler]
impl ServerHandler for AskMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: SERVER_NAME.into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Implementation::from_build_env()
            },
            instructions: Some(
                "ask_user asks the user a question in Marsad and returns the answer.".into(),
            ),
            ..ServerInfo::default()
        }
    }
}

/// `--socket <path> --session <id>`.
fn parse(args: &[String]) -> Result<(PathBuf, String), String> {
    let (mut socket, mut session) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--socket" => socket = Some(PathBuf::from(val()?)),
            "--session" => session = Some(val()?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok((
        socket.ok_or("--socket <path> is required")?,
        session.ok_or("--session <id> is required")?,
    ))
}

/// The `ask-mcp` subcommand (builds its own runtime).
pub fn run(args: &[String]) -> std::process::ExitCode {
    let (socket, session) = match parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("marsad-agent ask-mcp: {e}");
            return std::process::ExitCode::from(2);
        }
    };
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("marsad-agent ask-mcp: cannot start the runtime: {e}");
            return std::process::ExitCode::from(1);
        }
    };
    let r: Result<(), String> = rt.block_on(async move {
        use rmcp::ServiceExt;
        let endpoint = local_endpoint(&socket);
        let hello = Hello::new(
            concat!("marsad-ask-mcp/", env!("CARGO_PKG_VERSION")),
            "marsad-ask-mcp",
            "ask-mcp",
        );
        let link = Supervised::spawn(
            connector(move || {
                let endpoint = endpoint.clone();
                let opts = ClientOptions::new(hello.clone());
                async move { ControlClient::connect_endpoint(&endpoint, opts).await }
            }),
            SupervisedOptions::default(),
        );
        let handler = AskMcp {
            link,
            session,
            tool_router: AskMcp::tool_router(),
        };
        let running = handler
            .serve(rmcp::transport::io::stdio())
            .await
            .map_err(|e| format!("cannot start the MCP server: {e}"))?;
        running
            .waiting()
            .await
            .map(|_| ())
            .map_err(|e| format!("MCP server ended with an error: {e}"))
    });
    match r {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("marsad-agent ask-mcp: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(kind: Option<AskKind>, options: &[&str]) -> AskArgs {
        AskArgs {
            question: "Which?".into(),
            header: Some("Pick".into()),
            kind,
            options: options
                .iter()
                .map(|l| AskOption {
                    label: (*l).into(),
                    description: None,
                })
                .collect(),
            allow_other: None,
        }
    }

    #[test]
    fn every_kind_maps_to_a_native_question() {
        let q = args(None, &["a", "b"]).prompt().unwrap();
        assert!(!q.multi_select && !q.allow_other && q.options.len() == 2);
        let q = args(Some(AskKind::Multiple), &["a", "b"]).prompt().unwrap();
        assert!(q.multi_select);
        let q = args(None, &[]).prompt().unwrap();
        assert!(q.allow_other && q.options.is_empty(), "free text");
        let q = args(Some(AskKind::YesNo), &[]).prompt().unwrap();
        let labels: Vec<_> = q.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["Yes", "No"]);
        assert!(args(Some(AskKind::Single), &[]).prompt().is_err());
        assert_eq!(q.header, "Pick");
    }

    #[test]
    fn args_and_config() {
        let a: Vec<String> = ["--socket", "/d/s.sock", "--session", "abc"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            parse(&a).unwrap(),
            (PathBuf::from("/d/s.sock"), "abc".to_string())
        );
        assert!(parse(&a[..2]).is_err());
        let c = mcp_server_config("/bin/marsad-agent", "/d/s.sock", "abc");
        assert!(c.trusted);
        assert!(
            matches!(c.transport, McpTransport::Stdio { ref args, .. } if args[0] == "ask-mcp")
        );
    }

    /// A link that plays a script of `send` results. `up` says whether `connected` succeeds at
    /// once; when it does not, `connected` waits out its limit, as the real link does.
    struct FakeLink {
        up: bool,
        script: std::sync::Mutex<std::collections::VecDeque<Result<AskUserReply, CallError>>>,
        log: std::sync::Mutex<Vec<String>>,
    }

    impl FakeLink {
        fn new(up: bool, script: Vec<Result<AskUserReply, CallError>>) -> FakeLink {
            FakeLink {
                up,
                script: std::sync::Mutex::new(script.into()),
                log: Default::default(),
            }
        }
        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl AskLink for FakeLink {
        async fn connected(&self, max: Duration) -> Result<(), CallError> {
            self.log.lock().unwrap().push("connected".into());
            if self.up {
                return Ok(());
            }
            tokio::time::sleep(max).await;
            Err(CallError::NotConnected)
        }

        async fn send(
            &self,
            req: &AskUserRequest,
            _deadline: Duration,
        ) -> Result<AskUserReply, CallError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("send {}", req.request_id));
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(CallError::Timeout))
        }
    }

    fn ask_request() -> AskUserRequest {
        AskUserRequest {
            session: "abcdefghijkl".into(),
            request_id: "ask-0123456789abcdef".into(),
            question: args(None, &["a", "b"]).prompt().unwrap(),
        }
    }

    fn answered(label: &str) -> AskUserReply {
        AskUserReply {
            answers: vec![label.into()],
            cancelled: None,
        }
    }

    fn dropped() -> CallError {
        CallError::ConnectionLost(wasit_control::ConnClose::Eof)
    }

    #[tokio::test]
    async fn a_call_is_re_sent_with_its_request_id_after_the_connection_drops() {
        let link = FakeLink::new(true, vec![Err(dropped()), Ok(answered("b"))]);
        let reply = ask_until_answered(&link, &ask_request(), Duration::from_secs(60))
            .await
            .expect("the owner's answer");
        assert_eq!(reply.answers, ["b"]);
        // The retry waited for the link to be connected again before it was sent.
        assert_eq!(
            link.log(),
            [
                "connected",
                "send ask-0123456789abcdef",
                "connected",
                "send ask-0123456789abcdef",
            ]
        );
    }

    #[tokio::test]
    async fn drops_are_not_counted_only_the_deadline_ends_the_wait() {
        let mut script: Vec<_> = (0..7).map(|_| Err(dropped())).collect();
        script.push(Ok(answered("a")));
        let link = FakeLink::new(true, script);
        let reply = ask_until_answered(&link, &ask_request(), Duration::from_secs(60))
            .await
            .expect("the owner's answer after seven drops");
        assert_eq!(reply.answers, ["a"]);
        assert_eq!(
            link.log().iter().filter(|l| l.starts_with("send")).count(),
            8
        );
    }

    #[tokio::test]
    async fn failure_is_reported_only_when_the_deadline_passes() {
        let link = FakeLink::new(false, vec![]);
        let started = Instant::now();
        let deadline = Duration::from_millis(150);
        let err = ask_until_answered(&link, &ask_request(), deadline)
            .await
            .expect_err("no link, so no answer");
        assert!(started.elapsed() >= deadline, "gave up before the deadline");
        assert_eq!(
            err,
            "no answer within 150ms; the connection to Marsad was lost and not restored in time"
        );
        assert!(
            link.log().iter().all(|l| l == "connected"),
            "nothing was sent without a connection: {:?}",
            link.log()
        );
    }
}
