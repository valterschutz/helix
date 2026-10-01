use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context as _};
use helix_core::{
    unicode::width::UnicodeWidthStr, Position, Range, Selection, Tendril, Transaction,
};
use helix_view::{
    document::Mode,
    editor::FilePickerConfig,
    graphics::{CursorKind, Margin, Rect},
    DocumentId, Editor, ViewId,
};
use serde::{Deserialize, Serialize};
use tui::{
    buffer::Buffer as Surface,
    text::{Span, Text},
    widgets::{Block, Paragraph, Widget},
};

use crate::{
    compositor::{Component, Compositor, Context, Event, EventResult},
    ctrl, job,
    job::Jobs,
    key, shift,
    ui::{Prompt, PromptEvent},
};

use self::{
    request::{
        apply_completion, command_candidates, completion_token, normalize_request,
        reference_candidates, CommandInfo, TokenKind,
    },
    rpc::{PiProcess, SpawnOptions},
};

mod request;
mod rpc;

pub const ID: &str = "ai-chat";

const SYSTEM_PROMPT: &str = r#"You are an inline code transformation engine working on one selected snippet, given below. Each user request asks for a change to that snippet; later requests refine your previous replacement. Requests may name workspace files with a leading `@` (for example `@src/main.rs`): read those files with the read tool before answering. Respond with exactly one JSON object and no Markdown fences or prose: {"replacement":"the complete replacement snippet"}. The replacement must contain the entire snippet, including unchanged code. Never edit files or return a patch."#;
const MAX_INPUT_HEIGHT: u16 = 8;
const MAX_COMPLETION_ROWS: u16 = 8;
const MAX_WORKSPACE_FILES: usize = 50_000;
const DEFAULT_THINKING_LEVEL: ThinkingLevel = ThinkingLevel::Medium;
const ALL_THINKING_LEVELS: [ThinkingLevel; 7] = [
    ThinkingLevel::Off,
    ThinkingLevel::Minimal,
    ThinkingLevel::Low,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
    ThinkingLevel::Xhigh,
    ThinkingLevel::Max,
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingLevel {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct AiPreferences {
    model: String,
    thinking: ThinkingLevel,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PiSettings {
    default_provider: Option<String>,
    default_model: Option<String>,
    default_thinking_level: Option<ThinkingLevel>,
    enabled_models: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct CatalogProvider {
    models: Vec<CatalogModel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogModel {
    id: String,
    #[serde(default)]
    reasoning: bool,
    #[serde(default)]
    thinking_level_map: HashMap<String, Option<String>>,
}

struct AiConfiguration {
    preferences: AiPreferences,
    scoped_models: Vec<String>,
    thinking_levels: HashMap<String, Vec<ThinkingLevel>>,
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

/// One entry offered for the reference or command token under the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CompletionItem {
    /// Text that replaces the token when accepted, including the sigil.
    value: String,
    description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompletionMenu {
    /// Byte offset of the token's sigil in the request.
    token_start: usize,
    items: Vec<CompletionItem>,
    selected: usize,
}

pub struct AiChat {
    document_id: DocumentId,
    view_id: ViewId,
    range: Range,
    document_version: i32,
    original: String,
    proposed: Option<String>,
    cwd: PathBuf,
    process: Arc<PiProcess>,
    commands: Vec<CommandInfo>,
    workspace_files: Vec<String>,
    activity: Option<String>,
    completion: Option<CompletionMenu>,
    prompt: Prompt,
    phase: Phase,
    preferences: AiPreferences,
    scoped_models: Vec<String>,
    thinking_levels: HashMap<String, Vec<ThinkingLevel>>,
    scroll: u16,
    prompt_cursor: Option<Position>,
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
    ) -> anyhow::Result<Self> {
        assert!(
            !original.is_empty(),
            "AI edit requires a non-empty selection"
        );
        assert!(
            !cwd.as_os_str().is_empty(),
            "AI edit requires a working directory"
        );

        let ai_configuration = load_ai_configuration();
        let process = PiProcess::spawn(SpawnOptions {
            cwd: cwd.clone(),
            agent_dir: pi_agent_dir(),
            system_prompt: system_prompt_for(&original, &language, &file),
            model: ai_configuration.preferences.model.clone(),
            thinking: ai_configuration.preferences.thinking.as_str(),
        })?;

        Ok(Self {
            document_id,
            view_id,
            range,
            document_version,
            original,
            proposed: None,
            cwd,
            process,
            commands: Vec::new(),
            workspace_files: Vec::new(),
            activity: None,
            completion: None,
            prompt: Prompt::new(
                "".into(),
                None,
                |_, _| Vec::new(),
                |_, _, _: PromptEvent| {},
            ),
            phase: Phase::Input,
            preferences: ai_configuration.preferences,
            scoped_models: ai_configuration.scoped_models,
            thinking_levels: ai_configuration.thinking_levels,
            scroll: 0,
            prompt_cursor: None,
        })
    }

    /// Fetches pi's command list and walks the workspace for reference
    /// completion. Both arrive later through the job queue.
    pub fn load_resources(&self, editor: &Editor, jobs: &mut Jobs) {
        let process = Arc::clone(&self.process);
        jobs.callback(async move {
            let result = process.commands().await;
            Ok(job::Callback::EditorCompositor(Box::new(
                move |editor, compositor| {
                    let Some(chat) = compositor.find_id::<AiChat>(ID) else {
                        return;
                    };
                    match result {
                        Ok(commands) => chat.commands = commands,
                        Err(error) => {
                            editor.set_error(format!("pi commands unavailable: {error:#}"))
                        }
                    }
                },
            )))
        });

        let root = self.cwd.clone();
        let config = editor.config().file_picker.clone();
        jobs.callback(async move {
            let files = tokio::task::spawn_blocking(move || walk_workspace(&root, &config))
                .await
                .context("workspace walk panicked")?;
            Ok(job::Callback::EditorCompositor(Box::new(
                move |_, compositor| {
                    if let Some(chat) = compositor.find_id::<AiChat>(ID) {
                        chat.workspace_files = files;
                    }
                },
            )))
        });
    }

    fn submit(&mut self, cx: &mut Context) {
        let request = self.prompt.line().trim().to_owned();
        if request.is_empty() {
            cx.editor.set_error("Enter a message for pi");
            return;
        }

        let command_names: Vec<String> = self
            .commands
            .iter()
            .map(|command| command.name.clone())
            .collect();
        let message = normalize_request(&request, &command_names);
        let process = Arc::clone(&self.process);

        self.phase = Phase::Waiting;
        self.activity = None;
        self.completion = None;
        self.prompt.set_line(String::new(), cx.editor);

        cx.jobs.callback(async move {
            let result = process
                .prompt(message, |activity| {
                    tokio::spawn(job::dispatch(move |_, compositor| {
                        if let Some(chat) = compositor.find_id::<AiChat>(ID) {
                            chat.activity = Some(activity);
                        }
                    }));
                })
                .await
                .and_then(|text| parse_response(&text));
            Ok(job::Callback::EditorCompositor(Box::new(
                move |editor, compositor| {
                    let Some(chat) = compositor.find_id::<AiChat>(ID) else {
                        return;
                    };
                    chat.finish_request(result, editor);
                },
            )))
        });
    }

    fn finish_request(&mut self, result: anyhow::Result<String>, editor: &mut Editor) {
        self.activity = None;
        match result {
            Ok(replacement) => {
                self.proposed = Some(replacement);
                self.phase = Phase::Review;
                self.scroll = 0;
            }
            Err(error) => {
                let message = format!("pi failed: {error:#}");
                editor.set_error(message);
                self.phase = if self.proposed.is_some() {
                    Phase::Review
                } else {
                    Phase::Input
                };
            }
        }
    }

    /// Recomputes the completion menu for the token under the cursor.
    fn refresh_completion(&mut self, editor: &Editor) {
        let before_cursor = &self.prompt.line()[..self.prompt.position()];
        let Some(token) = completion_token(before_cursor) else {
            self.completion = None;
            return;
        };
        let items: Vec<CompletionItem> = match token.kind {
            TokenKind::Reference => {
                let open_buffers = open_buffer_paths(editor, &self.cwd);
                let current_file = editor
                    .documents
                    .get(&self.document_id)
                    .and_then(|document| relative_path(document, &self.cwd));
                reference_candidates(
                    &token.query,
                    &self.workspace_files,
                    &open_buffers,
                    current_file.as_deref(),
                )
                .into_iter()
                .map(|path| CompletionItem {
                    value: format!("@{path}"),
                    description: None,
                })
                .collect()
            }
            TokenKind::Command => command_candidates(&token.query, &self.commands)
                .into_iter()
                .map(|command| CompletionItem {
                    value: format!("/{}", command.name),
                    description: command.description.clone(),
                })
                .collect(),
        };
        self.completion = (!items.is_empty()).then_some(CompletionMenu {
            token_start: token.start,
            items,
            selected: 0,
        });
    }

    fn move_completion_selection(&mut self, forward: bool) {
        if let Some(menu) = &mut self.completion {
            let count = menu.items.len();
            menu.selected = if forward {
                (menu.selected + 1) % count
            } else {
                (menu.selected + count - 1) % count
            };
        }
    }

    /// Replaces the token under the cursor with the selected item and a space.
    fn accept_completion(&mut self, editor: &Editor) {
        let Some(menu) = self.completion.take() else {
            return;
        };
        let (line, cursor) = apply_completion(
            self.prompt.line(),
            self.prompt.position(),
            menu.token_start,
            &menu.items[menu.selected].value,
        );
        self.prompt.set_line_with_cursor(line, cursor, editor);
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

    fn input_height(&self) -> u16 {
        u16::try_from(self.prompt.line().split('\n').count())
            .unwrap_or(u16::MAX)
            .clamp(1, MAX_INPUT_HEIGHT)
    }

    fn positioned_area(&self, viewport: Rect, editor: &Editor) -> Rect {
        let maximum_content = (
            viewport.width.saturating_sub(2),
            viewport.height.saturating_sub(2),
        );
        let (content_width, content_height) = required_dimensions(
            self.phase,
            maximum_content,
            self.input_height(),
            self.proposed.is_some(),
        );
        let size = (
            content_width.saturating_add(2).min(viewport.width),
            content_height.saturating_add(2).min(viewport.height),
        );
        position_near_selection(viewport, editor.cursor().0.unwrap_or_default(), size)
    }

    fn render_code(&self, area: Rect, surface: &mut Surface, cx: &Context) {
        let Some(proposed) = self.proposed.as_deref() else {
            return;
        };
        if area.area() > 0 {
            self.render_comparison(area, surface, cx, proposed);
        }
    }

    fn render_input(&mut self, area: Rect, surface: &mut Surface, cx: &Context) {
        self.prompt_cursor = None;
        if self.phase == Phase::Waiting || area.area() == 0 {
            return;
        }

        let input = self.prompt.line();
        let before_cursor = &input[..self.prompt.position()];
        let cursor_row = before_cursor.bytes().filter(|byte| *byte == b'\n').count() as u16;
        let cursor_col = before_cursor
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .width() as u16;
        let vertical_scroll = cursor_row.saturating_sub(area.height.saturating_sub(1));
        let horizontal_scroll = cursor_col.saturating_sub(area.width.saturating_sub(1));
        let text = Text::from(input.as_str());
        Paragraph::new(&text)
            .style(cx.editor.theme.get("ui.text"))
            .scroll((vertical_scroll, horizontal_scroll))
            .render(area, surface);
        self.prompt_cursor = Some(Position::new(
            area.y as usize + cursor_row.saturating_sub(vertical_scroll) as usize,
            area.x as usize + cursor_col.saturating_sub(horizontal_scroll) as usize,
        ));
    }

    fn render_comparison(&self, area: Rect, surface: &mut Surface, cx: &Context, proposed: &str) {
        let left_width = area.width.saturating_sub(1) / 2;
        let right_width = area.width.saturating_sub(left_width + 1);
        let left = Rect::new(area.x, area.y, left_width, area.height);
        let right = Rect::new(area.x + left_width + 1, area.y, right_width, area.height);

        let original = cx
            .editor
            .theme
            .try_get("ui.ai.original")
            .unwrap_or_else(|| cx.editor.theme.get("diff.minus"));
        let output = cx
            .editor
            .theme
            .try_get("ui.ai.output")
            .unwrap_or_else(|| cx.editor.theme.get("diff.plus"));
        let old_block = Block::bordered().title(" Old ").border_style(original);
        let new_block = Block::bordered().title(" New ").border_style(output);
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

    fn cycle_model(&mut self, editor: &mut Editor, jobs: &mut Jobs) {
        let Some(model) = next_scoped_model(&self.preferences.model, &self.scoped_models) else {
            editor.set_error("Pi has no scoped models configured");
            return;
        };
        self.preferences.model = model.to_owned();
        let supported = self.supported_thinking_levels();
        self.preferences.thinking = clamp_thinking_level(self.preferences.thinking, supported);
        self.persist_preferences(editor);
        self.sync_session_preferences(jobs);
    }

    fn cycle_thinking_level(&mut self, editor: &mut Editor, jobs: &mut Jobs) {
        self.preferences.thinking =
            next_thinking_level(self.preferences.thinking, self.supported_thinking_levels());
        self.persist_preferences(editor);
        self.sync_session_preferences(jobs);
    }

    /// Pushes the current model and thinking level to the live pi process.
    fn sync_session_preferences(&self, jobs: &mut Jobs) {
        let process = Arc::clone(&self.process);
        let preferences = self.preferences.clone();
        jobs.callback(async move {
            let result = async {
                process.set_model(&preferences.model).await?;
                process
                    .set_thinking_level(preferences.thinking.as_str())
                    .await
            }
            .await;
            Ok(job::Callback::Editor(Box::new(move |editor| {
                if let Err(error) = result {
                    editor.set_error(format!("pi could not switch model: {error:#}"));
                }
            })))
        });
    }

    fn supported_thinking_levels(&self) -> &[ThinkingLevel] {
        self.thinking_levels
            .get(&self.preferences.model)
            .map(Vec::as_slice)
            .unwrap_or(&ALL_THINKING_LEVELS)
    }

    fn persist_preferences(&self, editor: &mut Editor) {
        if let Err(error) = save_ai_preferences(&self.preferences) {
            editor.set_error(format!("Could not save AI edit preferences: {error:#}"));
        }
    }

    fn prompt_title(&self) -> String {
        let model = self
            .preferences
            .model
            .rsplit_once('/')
            .map_or(self.preferences.model.as_str(), |(_, model)| model);
        match &self.activity {
            Some(activity) if self.phase == Phase::Waiting => {
                format!(
                    " {model} · {} · {activity} ",
                    self.preferences.thinking.as_str()
                )
            }
            _ => format!(" {model} · {} ", self.preferences.thinking.as_str()),
        }
    }

    fn render_completion(
        &self,
        input_area: Rect,
        viewport: Rect,
        surface: &mut Surface,
        cx: &Context,
    ) {
        let Some(menu) = &self.completion else {
            return;
        };
        let rows = (menu.items.len() as u16).min(MAX_COMPLETION_ROWS);
        let below = input_area.bottom().saturating_add(1);
        let y = if below.saturating_add(rows) <= viewport.bottom() {
            below
        } else {
            input_area.y.saturating_sub(rows + 1)
        };
        let area = Rect::new(
            input_area.x.saturating_sub(1),
            y,
            input_area.width + 2,
            rows,
        )
        .intersection(viewport);
        if area.area() == 0 {
            return;
        }

        let theme = &cx.editor.theme;
        let menu_style = theme.get("ui.menu");
        let selected_style = theme.get("ui.menu.selected");
        let description_style = theme.get("ui.text.inactive");
        surface.clear_with(area, menu_style);

        let offset = menu.selected.saturating_sub(rows as usize - 1);
        for (row, (index, item)) in menu
            .items
            .iter()
            .enumerate()
            .skip(offset)
            .take(rows as usize)
            .enumerate()
        {
            let y = area.y + row as u16;
            let style = if index == menu.selected {
                selected_style
            } else {
                menu_style
            };
            if index == menu.selected {
                surface.set_style(Rect::new(area.x, y, area.width, 1), style);
            }
            let (end, _) = surface.set_stringn(
                area.x + 1,
                y,
                &item.value,
                area.width.saturating_sub(2) as usize,
                style,
            );
            if let Some(description) = &item.description {
                let x = end + 2;
                if x < area.right() {
                    surface.set_stringn(
                        x,
                        y,
                        description,
                        (area.right() - x) as usize,
                        style.patch(description_style),
                    );
                }
            }
        }
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
                let result = self.prompt.handle_event(event, cx);
                self.refresh_completion(cx.editor);
                return result;
            }
            Event::Resize(..) | Event::Mouse(_) => return EventResult::Consumed(None),
            _ => return EventResult::Consumed(None),
        };

        let completing = self.completion.is_some() && self.phase != Phase::Waiting;
        match key {
            key!(Esc) if completing => {
                self.completion = None;
                EventResult::Consumed(None)
            }
            key!(Tab) | key!(Enter) if completing => {
                self.accept_completion(cx.editor);
                EventResult::Consumed(None)
            }
            key!(Down) if completing => {
                self.move_completion_selection(true);
                EventResult::Consumed(None)
            }
            shift!(Tab) | key!(Up) if completing => {
                self.move_completion_selection(false);
                EventResult::Consumed(None)
            }
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
            key!(Tab) if self.phase != Phase::Waiting => {
                self.cycle_model(cx.editor, cx.jobs);
                EventResult::Consumed(None)
            }
            shift!(Tab) if self.phase != Phase::Waiting => {
                self.cycle_thinking_level(cx.editor, cx.jobs);
                EventResult::Consumed(None)
            }
            shift!(Enter) if self.phase != Phase::Waiting => {
                self.prompt.insert_str("\n", cx.editor);
                self.refresh_completion(cx.editor);
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
            _ => {
                let result = self.prompt.handle_event(event, cx);
                self.refresh_completion(cx.editor);
                result
            }
        }
    }

    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let area = self.positioned_area(viewport, cx.editor);
        if area.area() == 0 {
            return;
        }

        let background = cx.editor.theme.get("ui.popup");
        let prompt_border = if self.phase == Phase::Waiting {
            cx.editor
                .theme
                .try_get("ui.ai.waiting")
                .unwrap_or_else(|| cx.editor.theme.get("warning"))
        } else {
            cx.editor
                .theme
                .try_get("ui.ai.input")
                .unwrap_or_else(|| cx.editor.theme.get("ui.text.focus"))
        };
        let comparison_border = cx
            .editor
            .theme
            .try_get("ui.ai.comparison")
            .unwrap_or_else(|| cx.editor.theme.get("ui.text.focus"));
        let has_comparison = self.proposed.is_some();
        let prompt_title = self.prompt_title();
        let outer_border = if has_comparison {
            comparison_border
        } else {
            prompt_border
        };
        surface.clear_with(area, background);
        let mut outer = Block::bordered()
            .style(background)
            .border_style(outer_border);
        if !has_comparison {
            outer = outer.title(Span::styled(prompt_title.clone(), prompt_border));
        }
        let inner = outer.inner(area).inner(Margin::horizontal(1));
        outer.render(area, surface);

        let input_height = if self.phase == Phase::Waiting {
            1
        } else {
            self.input_height()
        };
        if has_comparison {
            let prompt_height = input_height.saturating_add(2).min(inner.height);
            self.render_code(inner.clip_bottom(prompt_height), surface, cx);
            let prompt_area = Rect::new(
                inner.x,
                inner.bottom().saturating_sub(prompt_height),
                inner.width,
                prompt_height,
            );
            let prompt_block = Block::bordered()
                .title(Span::styled(prompt_title, prompt_border))
                .style(background)
                .border_style(prompt_border);
            let input_area = prompt_block.inner(prompt_area).inner(Margin::horizontal(1));
            prompt_block.render(prompt_area, surface);
            self.render_input(input_area, surface, cx);
            self.render_completion(input_area, viewport, surface, cx);
        } else {
            self.render_input(inner, surface, cx);
            self.render_completion(inner, viewport, surface, cx);
        }
    }

    fn cursor(&self, _area: Rect, editor: &Editor) -> (Option<Position>, CursorKind) {
        if self.phase == Phase::Waiting {
            (None, CursorKind::Hidden)
        } else {
            (
                self.prompt_cursor,
                editor.config().cursor_shape.from_mode(Mode::Insert),
            )
        }
    }

    fn id(&self) -> Option<&'static str> {
        Some(ID)
    }
}

impl Drop for AiChat {
    fn drop(&mut self) {
        self.process.close();
    }
}

fn next_scoped_model<'a>(current: &str, scoped_models: &'a [String]) -> Option<&'a str> {
    if scoped_models.is_empty() {
        return None;
    }
    let next = scoped_models
        .iter()
        .position(|model| model == current)
        .map_or(0, |index| (index + 1) % scoped_models.len());
    Some(scoped_models[next].as_str())
}

fn next_thinking_level(current: ThinkingLevel, supported: &[ThinkingLevel]) -> ThinkingLevel {
    assert!(
        !supported.is_empty(),
        "a model must support at least one thinking level"
    );
    let next = supported
        .iter()
        .position(|level| *level == current)
        .map_or(0, |index| (index + 1) % supported.len());
    supported[next]
}

fn clamp_thinking_level(requested: ThinkingLevel, supported: &[ThinkingLevel]) -> ThinkingLevel {
    assert!(
        !supported.is_empty(),
        "a model must support at least one thinking level"
    );
    if supported.contains(&requested) {
        return requested;
    }

    let requested_index = ALL_THINKING_LEVELS
        .iter()
        .position(|level| *level == requested)
        .expect("all thinking levels must be ordered");
    ALL_THINKING_LEVELS[requested_index..]
        .iter()
        .chain(ALL_THINKING_LEVELS[..requested_index].iter().rev())
        .find(|level| supported.contains(level))
        .copied()
        .unwrap_or(supported[0])
}

fn reconcile_preferences(
    stored: Option<AiPreferences>,
    default: &AiPreferences,
    scoped_models: &[String],
) -> AiPreferences {
    let Some(mut preferences) = stored else {
        return default.clone();
    };
    if preferences.model != default.model
        && !scoped_models
            .iter()
            .any(|model| model == &preferences.model)
    {
        preferences.model = scoped_models
            .first()
            .cloned()
            .unwrap_or_else(|| default.model.clone());
    }
    preferences
}

fn pi_agent_dir() -> Option<PathBuf> {
    env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".pi/agent")))
        .or_else(|| env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".pi/agent")))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let contents = fs::read(path).ok()?;
    serde_json::from_slice(&contents).ok()
}

fn load_ai_configuration() -> AiConfiguration {
    let agent_dir = pi_agent_dir();
    let settings: PiSettings = agent_dir
        .as_deref()
        .and_then(|directory| read_json(&directory.join("settings.json")))
        .unwrap_or_default();
    let scoped_models = settings.enabled_models.unwrap_or_default();
    let model = match (settings.default_provider, settings.default_model) {
        (Some(provider), Some(model)) => format!("{provider}/{model}"),
        _ => scoped_models.first().cloned().unwrap_or_default(),
    };
    let default = AiPreferences {
        model,
        thinking: settings
            .default_thinking_level
            .unwrap_or(DEFAULT_THINKING_LEVEL),
    };
    let stored = read_json(&ai_preferences_path());
    let mut preferences = reconcile_preferences(stored, &default, &scoped_models);
    let thinking_levels = agent_dir
        .as_deref()
        .and_then(|directory| load_thinking_levels(&directory.join("models-store.json")))
        .unwrap_or_default();
    let supported = thinking_levels
        .get(&preferences.model)
        .map(Vec::as_slice)
        .unwrap_or(&ALL_THINKING_LEVELS);
    preferences.thinking = clamp_thinking_level(preferences.thinking, supported);

    AiConfiguration {
        preferences,
        scoped_models,
        thinking_levels,
    }
}

fn load_thinking_levels(path: &Path) -> Option<HashMap<String, Vec<ThinkingLevel>>> {
    let catalog: HashMap<String, CatalogProvider> = read_json(path)?;
    let mut levels = HashMap::new();
    for (provider, catalog) in catalog {
        for model in catalog.models {
            let supported = if model.reasoning {
                ALL_THINKING_LEVELS
                    .iter()
                    .copied()
                    .filter(|level| match level {
                        ThinkingLevel::Xhigh | ThinkingLevel::Max => model
                            .thinking_level_map
                            .get(level.as_str())
                            .is_some_and(Option::is_some),
                        _ => !matches!(model.thinking_level_map.get(level.as_str()), Some(None)),
                    })
                    .collect()
            } else {
                vec![ThinkingLevel::Off]
            };
            levels.insert(format!("{provider}/{}", model.id), supported);
        }
    }
    Some(levels)
}

fn ai_preferences_path() -> PathBuf {
    helix_loader::data_dir().join("ai-preferences.json")
}

fn save_ai_preferences(preferences: &AiPreferences) -> anyhow::Result<()> {
    let path = ai_preferences_path();
    let parent = path
        .parent()
        .context("AI preference path must have a parent directory")?;
    fs::create_dir_all(parent).context("could not create the Helix data directory")?;
    let contents = serde_json::to_vec_pretty(preferences)
        .context("could not serialize AI edit preferences")?;
    fs::write(path, contents).context("could not write AI edit preferences")
}

fn position_near_selection(viewport: Rect, anchor: Position, size: (u16, u16)) -> Rect {
    assert!(
        size.0 <= viewport.width,
        "popup width must fit the viewport"
    );
    assert!(
        size.1 <= viewport.height,
        "popup height must fit the viewport"
    );

    let anchor_x = u16::try_from(anchor.col).unwrap_or(u16::MAX);
    let anchor_y = u16::try_from(anchor.row).unwrap_or(u16::MAX);
    let x = anchor_x.clamp(viewport.x, viewport.right().saturating_sub(size.0));
    let below = anchor_y.saturating_add(1);
    let y = if below.saturating_add(size.1) <= viewport.bottom() {
        below
    } else {
        anchor_y.saturating_sub(size.1)
    }
    .clamp(viewport.y, viewport.bottom().saturating_sub(size.1));

    Rect::new(x, y, size.0, size.1)
}

fn required_dimensions(
    phase: Phase,
    viewport: (u16, u16),
    input_height: u16,
    has_comparison: bool,
) -> (u16, u16) {
    if has_comparison {
        return (viewport.0.min(118), viewport.1.min(24));
    }

    let height = if phase == Phase::Waiting {
        1
    } else {
        input_height.clamp(1, MAX_INPUT_HEIGHT)
    };
    (viewport.0.min(72), viewport.1.min(height))
}

fn system_prompt_for(original: &str, language: &str, file: &str) -> String {
    format!(
        "{SYSTEM_PROMPT}\n\n<context>\nFile: {file}\nLanguage: {language}\n<snippet>\n{original}\n</snippet>\n</context>\n"
    )
}

/// Workspace files relative to `root`, using the file picker's walker settings.
fn walk_workspace(root: &Path, config: &FilePickerConfig) -> Vec<String> {
    crate::ui::workspace_file_walker(root, config)
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if !entry.path().is_file() {
                return None;
            }
            let relative = entry.path().strip_prefix(root).ok()?;
            Some(relative.to_string_lossy().into_owned())
        })
        .take(MAX_WORKSPACE_FILES)
        .collect()
}

/// Paths of open buffers inside the workspace, relative to `root`.
fn open_buffer_paths(editor: &Editor, root: &Path) -> Vec<String> {
    editor
        .documents()
        .filter_map(|document| relative_path(document, root))
        .collect()
}

fn relative_path(document: &helix_view::Document, root: &Path) -> Option<String> {
    let path = document.path()?.strip_prefix(root).ok()?;
    Some(path.to_string_lossy().into_owned())
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

#[cfg(test)]
mod tests {
    use super::{
        next_scoped_model, next_thinking_level, parse_response, position_near_selection,
        reconcile_preferences, required_dimensions, system_prompt_for, AiPreferences, Phase,
        ThinkingLevel,
    };
    use helix_core::Position;
    use helix_view::graphics::Rect;

    #[test]
    fn input_is_a_compact_chat_box() {
        assert_eq!(
            required_dimensions(Phase::Input, (118, 24), 1, false),
            (72, 1)
        );
        assert_eq!(
            required_dimensions(Phase::Input, (118, 24), 3, false),
            (72, 3)
        );
        assert_eq!(
            required_dimensions(Phase::Waiting, (40, 1), 3, false),
            (40, 1)
        );
    }

    #[test]
    fn review_uses_available_popup_space() {
        assert_eq!(
            required_dimensions(Phase::Review, (118, 24), 3, true),
            (118, 24)
        );
    }

    #[test]
    fn comparison_stays_open_while_waiting_for_a_refinement() {
        assert_eq!(
            required_dimensions(Phase::Waiting, (118, 24), 1, true),
            (118, 24)
        );
    }

    #[test]
    fn chat_is_positioned_below_the_selection() {
        let viewport = Rect::new(0, 0, 100, 30);

        assert_eq!(
            position_near_selection(viewport, Position::new(5, 10), (74, 4)),
            Rect::new(10, 6, 74, 4)
        );
    }

    #[test]
    fn chat_stays_on_screen_near_the_bottom_right() {
        let viewport = Rect::new(0, 0, 100, 30);

        assert_eq!(
            position_near_selection(viewport, Position::new(28, 90), (74, 4)),
            Rect::new(26, 24, 74, 4)
        );
    }

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
    fn model_cycle_enters_and_wraps_the_pi_scope() {
        let scoped = vec![
            "openai-codex/gpt-6-sol".to_owned(),
            "openai-codex/gpt-6-luna".to_owned(),
            "openai-codex/gpt-6-astra".to_owned(),
        ];

        assert_eq!(
            next_scoped_model("openai-codex/gpt-5.5", &scoped),
            Some("openai-codex/gpt-6-sol")
        );
        assert_eq!(
            next_scoped_model("openai-codex/gpt-6-sol", &scoped),
            Some("openai-codex/gpt-6-luna")
        );
        assert_eq!(
            next_scoped_model("openai-codex/gpt-6-astra", &scoped),
            Some("openai-codex/gpt-6-sol")
        );
    }

    #[test]
    fn thinking_cycle_uses_only_levels_supported_by_the_model() {
        let supported = [
            ThinkingLevel::Off,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ];

        assert_eq!(
            next_thinking_level(ThinkingLevel::Off, &supported),
            ThinkingLevel::Low
        );
        assert_eq!(
            next_thinking_level(ThinkingLevel::High, &supported),
            ThinkingLevel::Off
        );
    }

    #[test]
    fn saved_pi_default_is_valid_outside_the_scope_but_stale_models_are_not() {
        let default = AiPreferences {
            model: "openai-codex/gpt-5.5".to_owned(),
            thinking: ThinkingLevel::Medium,
        };
        let scoped = vec!["openai-codex/gpt-6-sol".to_owned()];

        assert_eq!(
            reconcile_preferences(Some(default.clone()), &default, &scoped),
            default
        );
        assert_eq!(
            reconcile_preferences(
                Some(AiPreferences {
                    model: "openai-codex/retired".to_owned(),
                    thinking: ThinkingLevel::High,
                }),
                &default,
                &scoped,
            ),
            AiPreferences {
                model: "openai-codex/gpt-6-sol".to_owned(),
                thinking: ThinkingLevel::High,
            }
        );
    }

    #[test]
    fn system_prompt_carries_the_snippet_and_its_origin() {
        let prompt = system_prompt_for("let value = 1;", "rust", "src/main.rs");

        assert!(prompt.contains("<snippet>\nlet value = 1;\n</snippet>"));
        assert!(prompt.contains("File: src/main.rs"));
        assert!(prompt.contains("Language: rust"));
        assert!(prompt.contains("leading `@`"));
    }
}
