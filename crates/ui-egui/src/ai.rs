use std::sync::mpsc::{Receiver, TryRecvError};

use serde_json::json;
use wordcraft_doc::StoryRef;
use wordcraft_engine::{Selection, Session};

use crate::WordApp;

const MAX_CONTEXT_CHARS: usize = 18_000;
const MAX_SUGGESTION_CONTEXT_CHARS: usize = 2_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Task {
    SuggestWord,
    ContinueDocument,
    WriteDocument,
}

struct Target {
    document_id: u64,
    revision: u64,
    selection: Selection,
}

#[derive(Default)]
pub(crate) struct AiWriter {
    pub(crate) open: bool,
    instruction: String,
    output: String,
    error: Option<String>,
    pending: Option<Receiver<Result<String, String>>>,
    task: Option<Task>,
    target: Option<Target>,
}

pub(crate) fn show(app: &mut WordApp, ctx: &egui::Context) {
    if let Some(receiver) = app.ai.pending.as_ref() {
        match receiver.try_recv() {
            Ok(Ok(text)) => {
                app.ai.pending = None;
                app.ai.output = text;
                app.ai.error = None;
            }
            Ok(Err(error)) => {
                app.ai.pending = None;
                app.ai.error = Some(error);
            }
            Err(TryRecvError::Disconnected) => {
                app.ai.pending = None;
                app.ai.error = Some("The AI request ended without a response.".into());
            }
            Err(TryRecvError::Empty) => {}
        }
    }
    if !app.ai.open {
        return;
    }

    let has_service = app.services.ai_complete.is_some();
    let waiting = app.ai.pending.is_some();
    let mut window_open = app.ai.open;
    let mut start = None;
    let mut apply = false;
    egui::Window::new(crate::tl!("AI writing"))
        .open(&mut window_open)
        .collapsible(false)
        .resizable(true)
        .default_width(560.0)
        .show(ctx, |ui| {
            ui.label(crate::tl!("Describe what you want to write, or ask for a word suggestion."));
            ui.add(
                egui::TextEdit::multiline(&mut app.ai.instruction)
                    .desired_rows(3)
                    .hint_text(crate::tl!("Writing instructions")),
            );
            ui.label(
                egui::RichText::new(crate::tl!(
                    "Your prompt and document context are sent to OpenRouter for processing."
                ))
                .small()
                .weak(),
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(has_service && !waiting, egui::Button::new(crate::tl!("Suggest word")))
                    .clicked()
                {
                    start = Some(Task::SuggestWord);
                }
                if ui
                    .add_enabled(has_service && !waiting, egui::Button::new(crate::tl!("Continue document")))
                    .clicked()
                {
                    start = Some(Task::ContinueDocument);
                }
                if ui
                    .add_enabled(has_service && !waiting, egui::Button::new(crate::tl!("Write document")))
                    .clicked()
                {
                    start = Some(Task::WriteDocument);
                }
            });
            if !has_service {
                ui.label(crate::tl!("AI writing is available in the desktop app, not in the browser."));
            }
            if waiting {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(crate::tl!("Waiting for OpenRouter…"));
                });
            }
            if let Some(error) = &app.ai.error {
                ui.colored_label(crate::theme::Tokens::get(ui.ctx()).red, error);
            }
            if !app.ai.output.is_empty() {
                ui.separator();
                ui.label(crate::tl!("Preview"));
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(&mut app.ai.output).desired_rows(8));
                });
                if ui.button(crate::tl!("Insert into document")).clicked() {
                    apply = true;
                }
            }
        });
    app.ai.open = window_open;

    if let Some(task) = start {
        submit(app, task);
    }
    if apply {
        apply_result(app);
    }
}

fn submit(app: &mut WordApp, task: Task) {
    let Some(request) = &app.services.ai_complete else {
        app.ai.error = Some("AI writing is not configured by this application host.".into());
        return;
    };
    let instruction = app.ai.instruction.trim();
    if task != Task::SuggestWord && instruction.is_empty() {
        app.ai.error = Some("Enter writing instructions first.".into());
        return;
    }

    let body = if task == Task::ContinueDocument {
        app.session.doc.plain_text(StoryRef::Body)
    } else {
        String::new()
    };
    let (paragraph, caret_off) = app
        .session
        .doc
        .para_at(&app.session.sel.focus)
        .map(|p| (p.text.clone(), app.session.sel.focus.off))
        .unwrap_or_default();
    let selected = app.session.selected_text();
    let prompt = match task {
        Task::SuggestWord => {
            let context = around(&paragraph, caret_off, MAX_SUGGESTION_CONTEXT_CHARS);
            let target = if selected.trim().is_empty() {
                "the next word or short phrase"
            } else {
                "a replacement for the selected text"
            };
            let request_text = if instruction.is_empty() {
                "Choose a natural, contextually appropriate word.".to_string()
            } else {
                instruction.into()
            };
            format!(
                "Suggest {target}. Return only the word or short phrase, with no explanation or quotation marks.\n\
                 User instruction: {request_text}\nSelected text: {selected}\nParagraph context:\n{context}"
            )
        }
        Task::ContinueDocument => {
            let context = tail(&body, MAX_CONTEXT_CHARS);
            let excerpt_note = if body.chars().count() > MAX_CONTEXT_CHARS {
                "Only the ending excerpt of this long document is included. "
            } else {
                ""
            };
            format!(
                "Continue the existing document. Write only new text that belongs after it; do not repeat its existing text. \
                 Begin on a new paragraph when appropriate.\nUser instruction: {instruction}\n{excerpt_note}Document ending:\n{context}"
            )
        }
        Task::WriteDocument =>
            format!(
                "Write a complete, polished document in response to the user's instructions. Return only the document text.\n\
                 User instruction: {instruction}"
            ),
    };
    app.ai.output.clear();
    app.ai.error = None;
    app.ai.task = Some(task);
    app.ai.target = Some(Target {
        document_id: app.session.document_id(),
        revision: app.session.rev(),
        selection: app.session.sel.clone(),
    });
    app.ai.pending = Some(request(prompt));
}

fn apply_result(app: &mut WordApp) {
    let (Some(task), Some(target)) = (app.ai.task, app.ai.target.take()) else {
        app.ai.error = Some("There is no completed AI response to insert.".into());
        return;
    };
    if app.session.document_id() != target.document_id || app.session.revision() != target.revision {
        app.ai.error = Some("The document changed while the AI was writing. Review the result, then run the request again.".into());
        return;
    }
    if task == Task::ContinueDocument {
        app.session.sel = Selection::caret(app.session.doc.end_of(StoryRef::Body));
    } else {
        app.session.sel = target.selection;
    }
    match app.session.run("text.insert", &json!({"text": app.ai.output})) {
        Ok(_) => {
            app.ai.output.clear();
            app.ai.task = None;
            app.ai.error = None;
        }
        Err(error) => app.ai.error = Some(format!("Couldn't insert the AI text: {error}")),
    }
}

fn tail(text: &str, limit: usize) -> String {
    text.chars().rev().take(limit).collect::<String>().chars().rev().collect()
}

fn around(text: &str, offset: usize, limit: usize) -> String {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset = offset.saturating_sub(1);
    }
    let before_limit = limit / 2;
    let before = text.get(..offset).unwrap_or_default().chars().rev().take(before_limit).collect::<String>();
    let before = before.chars().rev().collect::<String>();
    let after = text
        .get(offset..)
        .unwrap_or_default()
        .chars()
        .take(limit.saturating_sub(before.chars().count()))
        .collect::<String>();
    format!("{before}{after}")
}

#[cfg(test)]
mod tests {
    use super::{around, tail};

    #[test]
    fn context_tail_is_character_safe_and_bounded() {
        assert_eq!(tail("aé🙂c", 2), "🙂c");
        assert_eq!(tail("short", 20), "short");
    }

    #[test]
    fn suggestion_context_is_centered_on_the_caret() {
        assert_eq!(around("0123456789", 5, 4), "3456");
        assert_eq!(around("aé🙂c", 3, 3), "é🙂c");
    }
}
