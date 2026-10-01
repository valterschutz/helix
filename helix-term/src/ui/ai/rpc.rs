//! A pi process in RPC mode that lives for one AI edit conversation.
//!
//! Pi owns the conversation history and reads references with its `read`
//! tool. Helix sends normalised requests and parses the final assistant text.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use anyhow::{bail, Context as _};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, oneshot},
    time::timeout,
};

use super::request::CommandInfo;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(300);
const CLAUDE_BRIDGE_PACKAGE: &str = "npm/node_modules/pi-claude-bridge";

/// What Helix needs to start a pi process for one conversation.
pub struct SpawnOptions {
    pub cwd: PathBuf,
    pub agent_dir: Option<PathBuf>,
    pub system_prompt: String,
    pub model: String,
    pub thinking: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// A tool started running; the text describes it for the user.
    ToolStarted(String),
    /// The assistant finished a message: its text, or the provider's error.
    AssistantMessage(Result<String, String>),
    /// Pi has no more automatic work for the current request.
    Settled,
    /// Pi's stdout closed.
    Exited,
    Other,
}

pub struct PiSession {
    stdin: tokio::sync::Mutex<ChildStdin>,
    child: Mutex<Option<Child>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    events: tokio::sync::Mutex<mpsc::UnboundedReceiver<SessionEvent>>,
    next_id: AtomicU64,
    system_prompt_file: PathBuf,
}

#[derive(Deserialize)]
struct CommandsData {
    commands: Vec<RpcCommand>,
}

#[derive(Deserialize)]
struct RpcCommand {
    name: String,
    description: Option<String>,
    source: String,
}

impl PiSession {
    /// Starts pi. Must be called from within the tokio runtime.
    pub fn spawn(options: SpawnOptions) -> anyhow::Result<Arc<Self>> {
        let system_prompt_file = write_system_prompt_file(&options.system_prompt)?;

        let mut command = Command::new("pi");
        command.current_dir(&options.cwd).args([
            "--mode",
            "rpc",
            "--no-session",
            "--no-extensions",
            "--no-context-files",
            "--tools",
            "read",
        ]);
        if let Some(extension) = claude_bridge_extension(options.agent_dir.as_deref()) {
            command.arg("--extension").arg(extension);
        }
        command.arg("--system-prompt").arg(&system_prompt_file);
        if !options.model.is_empty() {
            command.args(["--model", &options.model]);
        }
        command
            .args(["--thinking", options.thinking])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        let mut child = command.spawn().context("could not start `pi`")?;
        let stdin = child.stdin.take().expect("pi stdin must be piped");
        let stdout = child.stdout.take().expect("pi stdout must be piped");
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> = Arc::default();
        let (event_sender, event_receiver) = mpsc::unbounded_channel();

        let reader_pending = Arc::clone(&pending);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(record) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if record["type"] == "response" {
                    let id = record["id"].as_str().and_then(|id| id.parse().ok());
                    let sender = id.and_then(|id| reader_pending.lock().unwrap().remove(&id));
                    if let Some(sender) = sender {
                        let _ = sender.send(record);
                    }
                } else if event_sender.send(classify_event(&record)).is_err() {
                    break;
                }
            }
            let _ = event_sender.send(SessionEvent::Exited);
        });

        Ok(Arc::new(Self {
            stdin: tokio::sync::Mutex::new(stdin),
            child: Mutex::new(Some(child)),
            pending,
            events: tokio::sync::Mutex::new(event_receiver),
            next_id: AtomicU64::new(1),
            system_prompt_file,
        }))
    }

    /// Skills and prompt templates pi loaded, in pi's order.
    pub async fn commands(&self) -> anyhow::Result<Vec<CommandInfo>> {
        let response = self.command(json!({"type": "get_commands"})).await?;
        let data: CommandsData = serde_json::from_value(response["data"].clone())
            .context("pi returned an unexpected command list")?;
        Ok(data
            .commands
            .into_iter()
            .filter(|command| command.source == "skill" || command.source == "prompt")
            .map(|command| CommandInfo {
                name: command.name,
                description: command.description,
            })
            .collect())
    }

    pub async fn set_model(&self, model: &str) -> anyhow::Result<()> {
        let (provider, model_id) =
            split_model(model).with_context(|| format!("`{model}` is not provider/model"))?;
        self.command(json!({"type": "set_model", "provider": provider, "modelId": model_id}))
            .await
            .map(drop)
    }

    pub async fn set_thinking_level(&self, level: &str) -> anyhow::Result<()> {
        self.command(json!({"type": "set_thinking_level", "level": level}))
            .await
            .map(drop)
    }

    /// Sends a request and waits for pi to settle. `on_tool` is called with a
    /// description of each tool call as it starts. Returns the text of the last
    /// assistant message.
    pub async fn prompt(
        &self,
        message: String,
        mut on_tool: impl FnMut(String),
    ) -> anyhow::Result<String> {
        let mut events = self.events.lock().await;
        while events.try_recv().is_ok() {}

        self.command(json!({"type": "prompt", "message": message}))
            .await
            .context("pi rejected the request")?;

        let wait = async {
            let mut last: Option<Result<String, String>> = None;
            loop {
                match events.recv().await {
                    Some(SessionEvent::ToolStarted(activity)) => on_tool(activity),
                    Some(SessionEvent::AssistantMessage(result)) => last = Some(result),
                    Some(SessionEvent::Settled) => break,
                    Some(SessionEvent::Other) => {}
                    Some(SessionEvent::Exited) | None => bail!("pi exited unexpectedly"),
                }
            }
            match last {
                Some(Ok(text)) => Ok(text),
                Some(Err(error)) => bail!("{error}"),
                None => bail!("pi returned no answer"),
            }
        };
        match timeout(RESPONSE_TIMEOUT, wait).await {
            Ok(result) => result,
            Err(_) => {
                let _ = self.command(json!({"type": "abort"})).await;
                bail!("pi did not respond within five minutes")
            }
        }
    }

    pub fn close(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.start_kill();
        }
        let _ = fs::remove_file(&self.system_prompt_file);
    }

    async fn command(&self, mut command: Value) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        command["id"] = Value::String(id.to_string());
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);

        let mut line = serde_json::to_string(&command).expect("commands are plain JSON");
        line.push('\n');
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(line.as_bytes())
                .await
                .context("could not write to pi")?;
        }

        let response = receiver.await.context("pi exited before responding")?;
        if response["success"] == true {
            return Ok(response);
        }
        let error = response["error"].as_str().unwrap_or("unknown error");
        bail!("{error}")
    }
}

impl Drop for PiSession {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn split_model(model: &str) -> Option<(&str, &str)> {
    model.split_once('/')
}

pub fn classify_event(record: &Value) -> SessionEvent {
    match record["type"].as_str() {
        Some("agent_settled") => SessionEvent::Settled,
        Some("tool_execution_start") => {
            let tool = record["toolName"].as_str().unwrap_or("tool");
            let subject = record["args"]["path"]
                .as_str()
                .or_else(|| record["args"]["command"].as_str())
                .unwrap_or_default();
            SessionEvent::ToolStarted(format!("{tool} {subject}").trim_end().to_owned())
        }
        Some("message_end") if record["message"]["role"] == "assistant" => {
            let message = &record["message"];
            let result = match message["stopReason"].as_str() {
                Some("error") | Some("aborted") => Err(message["errorMessage"]
                    .as_str()
                    .unwrap_or("pi stopped without an answer")
                    .to_owned()),
                _ => Ok(message["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|block| block["type"] == "text")
                    .filter_map(|block| block["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("")),
            };
            SessionEvent::AssistantMessage(result)
        }
        _ => SessionEvent::Other,
    }
}

fn claude_bridge_extension(agent_dir: Option<&Path>) -> Option<PathBuf> {
    #[derive(Deserialize)]
    struct Package {
        pi: Option<PiManifest>,
    }
    #[derive(Deserialize)]
    struct PiManifest {
        #[serde(default)]
        extensions: Vec<String>,
    }

    let package_dir = agent_dir?.join(CLAUDE_BRIDGE_PACKAGE);
    let manifest = fs::read(package_dir.join("package.json")).ok()?;
    let package: Package = serde_json::from_slice(&manifest).ok()?;
    let entry = package.pi?.extensions.into_iter().next()?;
    let path = package_dir.join(entry);
    path.is_file().then_some(path)
}

fn write_system_prompt_file(contents: &str) -> anyhow::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "helix-ai-edit-{}-{}.md",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, contents).context("could not write the AI edit system prompt")?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_text_is_taken_from_the_final_message() {
        let event: Value = serde_json::from_str(
            r#"{"type":"message_end","message":{"role":"assistant","stopReason":"stop","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"{\"replacement\":\"x\"}"}]}}"#,
        )
        .unwrap();

        assert_eq!(
            classify_event(&event),
            SessionEvent::AssistantMessage(Ok("{\"replacement\":\"x\"}".to_owned()))
        );
    }

    #[test]
    fn provider_errors_are_reported_from_the_final_message() {
        let event: Value = serde_json::from_str(
            r#"{"type":"message_end","message":{"role":"assistant","stopReason":"error","errorMessage":"rate limited","content":[]}}"#,
        )
        .unwrap();

        assert_eq!(
            classify_event(&event),
            SessionEvent::AssistantMessage(Err("rate limited".to_owned()))
        );
    }

    #[test]
    fn tool_starts_describe_the_activity() {
        let event: Value = serde_json::from_str(
            r#"{"type":"tool_execution_start","toolCallId":"1","toolName":"read","args":{"path":"src/x.py"}}"#,
        )
        .unwrap();

        assert_eq!(
            classify_event(&event),
            SessionEvent::ToolStarted("read src/x.py".to_owned())
        );
    }

    #[test]
    fn settling_and_user_messages_are_distinguished() {
        let settled: Value = serde_json::from_str(r#"{"type":"agent_settled"}"#).unwrap();
        let user: Value = serde_json::from_str(
            r#"{"type":"message_end","message":{"role":"user","content":"hi"}}"#,
        )
        .unwrap();

        assert_eq!(classify_event(&settled), SessionEvent::Settled);
        assert_eq!(classify_event(&user), SessionEvent::Other);
    }

    #[test]
    fn scoped_models_split_into_provider_and_id() {
        assert_eq!(
            split_model("claude-bridge/claude-opus-5-5"),
            Some(("claude-bridge", "claude-opus-5-5"))
        );
        assert_eq!(split_model("gpt-5.5"), None);
    }
}
