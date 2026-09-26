use std::{path::PathBuf, process::Stdio, time::Duration};

use anyhow::{bail, Context as _};
use helix_core::{Range, Selection, Tendril, Transaction};
use helix_view::{
    graphics::{CursorKind, Margin, Rect},
    DocumentId, Editor, ViewId,
};
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, process::Command, time::timeout};
use tui::{
    buffer::Buffer as Surface,
    text::Text,
    widgets::{Block, Paragraph, Widget},
};

use crate::{
    compositor::{Component, Compositor, Context, Event, EventResult},
    ctrl, job, key,
    ui::{Prompt, PromptEvent},
};

pub const ID: &str = "ai-chat";

const SYSTEM_PROMPT: &str = r#"You are an inline code transformation engine. Respond with exactly one JSON object and no Markdown fences or prose: {"replacement":"the complete replacement snippet"}. The replacement must contain the entire code snippet, including unchanged code. Apply the user's latest request while respecting the prior conversation. Never edit files or return a patch."#;
const PI_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Serialize)]
struct ConversationTurn {
    request: String,
    replacement: String,
}

#[derive(Serialize)]
struct RequestContext {
    original: String,
    current: String,
    language: String,
    file: String,
    conversation: Vec<ConversationTurn>,
    request: String,
}

#[derive(Deserialize)]
struct PiResponse {
    replacement: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Input,
    Waiting,
    Review,
}

pub struct AiChat {
    document_id: DocumentId,
    view_id: ViewId,
    range: Range,
    document_version: i32,
    original: String,
    proposed: Option<String>,
    language: String,
    file: String,
    cwd: PathBuf,
    conversation: Vec<ConversationTurn>,
    prompt: Prompt,
    prompt_area: Rect,
    phase: Phase,
    scroll: u16,
    error: Option<String>,
}

impl AiChat {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        document_id: DocumentId,
        view_id: ViewId,
        range: Range,
        document_version: i32,
        original: String,
        language: String,
        file: String,
        cwd: PathBuf,
    ) -> Self {
        assert!(
            !original.is_empty(),
            "AI edit requires a non-empty selection"
        );
        assert!(
            !cwd.as_os_str().is_empty(),
            "AI edit requires a working directory"
        );

        Self {
            document_id,
            view_id,
            range,
            document_version,
            original,
            proposed: None,
            language,
            file,
            cwd,
            conversation: Vec::new(),
            prompt: Prompt::new(
                "Message: ".into(),
                None,
                |_, _| Vec::new(),
                |_, _, _: PromptEvent| {},
            ),
            prompt_area: Rect::default(),
            phase: Phase::Input,
            scroll: 0,
            error: None,
        }
    }

    fn submit(&mut self, cx: &mut Context) {
        let request = self.prompt.line().trim().to_owned();
        if request.is_empty() {
            cx.editor.set_error("Enter a message for pi");
            return;
        }

        let context = RequestContext {
            original: self.original.clone(),
            current: self.proposed.as_ref().unwrap_or(&self.original).clone(),
            language: self.language.clone(),
            file: self.file.clone(),
            conversation: self.conversation.clone(),
            request: request.clone(),
        };
        let input = build_request(context);
        let cwd = self.cwd.clone();

        self.phase = Phase::Waiting;
        self.error = None;
        self.prompt.set_line(String::new(), cx.editor);

        cx.jobs.callback(async move {
            let result = run_pi(cwd, input).await;
            Ok(job::Callback::EditorCompositor(Box::new(
                move |editor, compositor| {
                    let Some(chat) = compositor.find_id::<AiChat>(ID) else {
                        return;
                    };
                    chat.finish_request(request, result, editor);
                },
            )))
        });
    }

    fn finish_request(
        &mut self,
        request: String,
        result: anyhow::Result<String>,
        editor: &mut Editor,
    ) {
        match result {
            Ok(replacement) => {
                self.conversation.push(ConversationTurn {
                    request,
                    replacement: replacement.clone(),
                });
                self.proposed = Some(replacement);
                self.phase = Phase::Review;
                self.scroll = 0;
                self.error = None;
            }
            Err(error) => {
                let message = format!("pi failed: {error:#}");
                editor.set_error(message.clone());
                self.error = Some(message);
                self.phase = if self.proposed.is_some() {
                    Phase::Review
                } else {
                    Phase::Input
                };
            }
        }
    }

    fn apply(&self, editor: &mut Editor) -> anyhow::Result<()> {
        let replacement = self
            .proposed
            .as_ref()
            .context("Wait for pi to propose a replacement")?;
        let (view, document) = helix_view::current!(editor);

        if document.id() != self.document_id || view.id != self.view_id {
            bail!("The edited view is no longer active; start the AI edit again");
        }
        if document.version() != self.document_version {
            bail!("The document changed while pi was working; start the AI edit again");
        }
        if self.range.slice(document.text().slice(..)) != self.original.as_str() {
            bail!("The selected code changed while pi was working; start the AI edit again");
        }

        let replacement_len = replacement.chars().count();
        let new_range = Range::new(self.range.from(), self.range.from() + replacement_len)
            .with_direction(self.range.direction());
        let transaction = Transaction::change(
            document.text(),
            std::iter::once((
                self.range.from(),
                self.range.to(),
                Some(Tendril::from(replacement.as_str())),
            )),
        )
        .with_selection(Selection::single(new_range.anchor, new_range.head));

        document.apply(&transaction, view.id);
        document.append_changes_to_history(view);
        Ok(())
    }

    fn modal_area(viewport: Rect) -> Rect {
        let width = viewport.width.saturating_sub(4).min(140);
        let height = viewport.height.saturating_sub(4).min(42);
        Rect::new(
            viewport.x + viewport.width.saturating_sub(width) / 2,
            viewport.y + viewport.height.saturating_sub(height) / 2,
            width,
            height,
        )
    }

    fn render_code(&self, area: Rect, surface: &mut Surface, cx: &Context) {
        if area.area() == 0 {
            return;
        }

        match self.proposed.as_deref() {
            Some(proposed) => self.render_comparison(area, surface, cx, proposed),
            None => {
                let block = Block::bordered()
                    .title(" Selected code ")
                    .border_style(cx.editor.theme.get("ui.text"));
                let inner = block.inner(area).inner(Margin::horizontal(1));
                block.render(area, surface);
                let text = Text::from(self.original.as_str());
                Paragraph::new(&text)
                    .style(cx.editor.theme.get("ui.text"))
                    .scroll((self.scroll, 0))
                    .render(inner, surface);
            }
        }
    }

    fn render_comparison(&self, area: Rect, surface: &mut Surface, cx: &Context, proposed: &str) {
        let left_width = area.width.saturating_sub(1) / 2;
        let right_width = area.width.saturating_sub(left_width + 1);
        let left = Rect::new(area.x, area.y, left_width, area.height);
        let right = Rect::new(area.x + left_width + 1, area.y, right_width, area.height);

        let old_block = Block::bordered()
            .title(" Old ")
            .border_style(cx.editor.theme.get("diff.minus"));
        let new_block = Block::bordered()
            .title(" New ")
            .border_style(cx.editor.theme.get("diff.plus"));
        let old_inner = old_block.inner(left).inner(Margin::horizontal(1));
        let new_inner = new_block.inner(right).inner(Margin::horizontal(1));
        old_block.render(left, surface);
        new_block.render(right, surface);

        let old_text = Text::from(self.original.as_str());
        let new_text = Text::from(proposed);
        Paragraph::new(&old_text)
            .style(cx.editor.theme.get("ui.text"))
            .scroll((self.scroll, 0))
            .render(old_inner, surface);
        Paragraph::new(&new_text)
            .style(cx.editor.theme.get("ui.text"))
            .scroll((self.scroll, 0))
            .render(new_inner, surface);
    }

    fn close_result() -> EventResult {
        EventResult::Consumed(Some(Box::new(|compositor: &mut Compositor, _| {
            compositor.remove(ID);
        })))
    }
}

impl Component for AiChat {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let key = match event {
            Event::Key(key) => *key,
            Event::Paste(_) if self.phase != Phase::Waiting => {
                return self.prompt.handle_event(event, cx)
            }
            Event::Resize(..) | Event::Mouse(_) => return EventResult::Consumed(None),
            _ => return EventResult::Consumed(None),
        };

        match key {
            key!(Esc) | ctrl!('c') => Self::close_result(),
            ctrl!('s') if self.phase == Phase::Review => match self.apply(cx.editor) {
                Ok(()) => {
                    cx.editor.set_status("Applied pi's replacement");
                    Self::close_result()
                }
                Err(error) => {
                    cx.editor.set_error(error.to_string());
                    EventResult::Consumed(None)
                }
            },
            ctrl!('s') => {
                cx.editor.set_error("Wait for pi to propose a replacement");
                EventResult::Consumed(None)
            }
            key!(Enter) if self.phase != Phase::Waiting => {
                self.submit(cx);
                EventResult::Consumed(None)
            }
            key!(PageUp) => {
                self.scroll = self.scroll.saturating_sub(10);
                EventResult::Consumed(None)
            }
            key!(PageDown) => {
                self.scroll = self.scroll.saturating_add(10);
                EventResult::Consumed(None)
            }
            _ if self.phase == Phase::Waiting => EventResult::Consumed(None),
            _ => self.prompt.handle_event(event, cx),
        }
    }

    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let area = Self::modal_area(viewport);
        if area.area() == 0 {
            return;
        }

        let background = cx.editor.theme.get("ui.popup");
        let focus = cx.editor.theme.get("ui.text.focus");
        surface.clear_with(area, background);
        let title = if self.phase == Phase::Waiting {
            " Pi edit · working… "
        } else {
            " Pi edit "
        };
        let outer = Block::bordered()
            .title(title)
            .style(background)
            .border_style(focus);
        let inner = outer.inner(area).inner(Margin::horizontal(1));
        outer.render(area, surface);

        let footer_height = 2.min(inner.height);
        let code_area = inner.clip_bottom(footer_height);
        self.render_code(code_area, surface, cx);

        self.prompt_area = Rect::new(
            inner.x,
            inner.bottom().saturating_sub(footer_height),
            inner.width,
            u16::from(footer_height > 0),
        );
        if self.prompt_area.area() > 0 {
            if self.phase == Phase::Waiting {
                surface.set_string(
                    self.prompt_area.x,
                    self.prompt_area.y,
                    "Waiting for pi…",
                    cx.editor.theme.get("ui.text.inactive"),
                );
            } else {
                self.prompt.render(self.prompt_area, surface, cx);
            }
        }

        if footer_height > 1 {
            let help = self.error.as_deref().unwrap_or(if self.proposed.is_some() {
                "enter send feedback · ctrl-s apply · page-up/down scroll · esc cancel"
            } else {
                "enter send · page-up/down scroll · esc cancel"
            });
            let style = if self.error.is_some() {
                cx.editor.theme.get("error")
            } else {
                cx.editor.theme.get("ui.text.inactive")
            };
            surface.set_stringn(
                inner.x,
                inner.bottom() - 1,
                help,
                inner.width as usize,
                style,
            );
        }
    }

    fn cursor(&self, _area: Rect, editor: &Editor) -> (Option<helix_core::Position>, CursorKind) {
        if self.phase == Phase::Waiting {
            (None, CursorKind::Hidden)
        } else {
            self.prompt.cursor(self.prompt_area, editor)
        }
    }

    fn id(&self) -> Option<&'static str> {
        Some(ID)
    }
}

fn build_request(context: RequestContext) -> String {
    serde_json::to_string(&context).expect("serializing an AI edit request cannot fail")
}

fn parse_response(output: &str) -> anyhow::Result<String> {
    let output = output.trim();
    if let Ok(response) = serde_json::from_str::<PiResponse>(output) {
        return Ok(response.replacement);
    }

    let start = output.find('{').context("pi returned no JSON object")?;
    let end = output.rfind('}').context("pi returned incomplete JSON")?;
    let response: PiResponse = serde_json::from_str(&output[start..=end])
        .context("pi returned invalid replacement JSON")?;
    Ok(response.replacement)
}

async fn run_pi(cwd: PathBuf, input: String) -> anyhow::Result<String> {
    let mut command = Command::new("pi");
    command
        .current_dir(cwd)
        .args([
            "--print",
            "--no-session",
            "--no-tools",
            "--no-extensions",
            "--no-skills",
            "--no-prompt-templates",
            "--no-context-files",
            "--system-prompt",
            SYSTEM_PROMPT,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn().context("could not start `pi`")?;
    let mut stdin = child.stdin.take().expect("pi stdin must be piped");
    stdin
        .write_all(input.as_bytes())
        .await
        .context("could not send the request to pi")?;
    drop(stdin);

    let output = timeout(PI_TIMEOUT, child.wait_with_output())
        .await
        .context("pi did not respond within five minutes")??;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if stderr.is_empty() {
            bail!("pi exited with {}", output.status);
        }
        bail!("pi exited with {}: {stderr}", output.status);
    }

    let stdout = String::from_utf8(output.stdout).context("pi returned non-UTF-8 output")?;
    parse_response(&stdout)
}

#[cfg(test)]
mod tests {
    use super::{build_request, parse_response, ConversationTurn, RequestContext};

    #[test]
    fn parses_json_response() {
        let response = parse_response(r#"{"replacement":"fn answer() {\n    42\n}"}"#).unwrap();

        assert_eq!(response, "fn answer() {\n    42\n}");
    }

    #[test]
    fn parses_json_response_wrapped_in_markdown() {
        let response =
            parse_response("```json\n{\"replacement\":\"let result = 42;\"}\n```").unwrap();

        assert_eq!(response, "let result = 42;");
    }

    #[test]
    fn request_contains_context_and_conversation() {
        let context = RequestContext {
            original: "let value = 1;".into(),
            current: "let value = 2;".into(),
            language: "rust".into(),
            file: "src/main.rs".into(),
            conversation: vec![ConversationTurn {
                request: "Increment the value".into(),
                replacement: "let value = 2;".into(),
            }],
            request: "Use a descriptive name".into(),
        };

        let request: serde_json::Value = serde_json::from_str(&build_request(context)).unwrap();

        assert_eq!(request["original"], "let value = 1;");
        assert_eq!(request["current"], "let value = 2;");
        assert_eq!(request["language"], "rust");
        assert_eq!(request["file"], "src/main.rs");
        assert_eq!(request["conversation"][0]["request"], "Increment the value");
        assert_eq!(request["request"], "Use a descriptive name");
    }
}
