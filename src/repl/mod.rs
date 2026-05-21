use crate::config::Config;
use crate::executor::Executor;
use crate::prompt::PromptRenderer;
use anyhow::Result;
use nu_ansi_term::{Color, Style};
use reedline::{
    default_emacs_keybindings, default_vi_insert_keybindings, default_vi_normal_keybindings,
    ColumnarMenu, EditCommand, EditMode, Emacs, KeyCode, KeyModifiers,
    MenuBuilder, FileBackedHistory, Prompt, PromptEditMode, PromptHistorySearch,
    PromptHistorySearchStatus, Reedline, ReedlineEvent, ReedlineMenu, Signal, Vi,
    DefaultHinter,
};
use std::borrow::Cow;

mod completer;
mod highlighter;

pub struct Shell {
    editor: Reedline,
    executor: Executor,
    prompt: ZishPrompt,
    renderer: PromptRenderer,
    config: Config,
}

impl Shell {
    pub fn new(config: Config) -> Result<Self> {
        // ── History — load from SQLite into reedline MemHistory ─────────────
        let mem_history = {
            let hist = crate::history::HISTORY.read().unwrap();
            Box::new(hist.to_reedline_history())
        };

        // ── Completions ──────────────────────────────────────────────────────
        let completer = Box::new(completer::ZishCompleter::new(&config));

        // ── Syntax highlighting ──────────────────────────────────────────────
        let highlighter = Box::new(highlighter::ZishHighlighter::new(&config));

        // ── Inline hints (greyed-out fish-style using history + completer) ───
        let hinter = Box::new(ZishHinter::new(&config));

        // ── Completion menu ──────────────────────────────────────────────────
        let completion_menu = Box::new(
            ColumnarMenu::default()
                .with_name("completion_menu")
                .with_columns(4)
                .with_column_padding(2),
        );

        // ── Keybindings ──────────────────────────────────────────────────────
        let edit_mode: Box<dyn EditMode> = match config.keybindings.mode.as_str() {
            "vi" => {
                let mut insert = default_vi_insert_keybindings();
                // Tab completion in vi insert mode
                insert.add_binding(
                    KeyModifiers::NONE,
                    KeyCode::Tab,
                    ReedlineEvent::UntilFound(vec![
                        ReedlineEvent::Menu("completion_menu".to_string()),
                        ReedlineEvent::MenuNext,
                    ]),
                );
                // Right Arrow → accept inline hint or move right
                insert.add_binding(
                    KeyModifiers::NONE,
                    KeyCode::Right,
                    ReedlineEvent::UntilFound(vec![
                        ReedlineEvent::HistoryHintComplete,
                        ReedlineEvent::Right,
                    ]),
                );
                Box::new(Vi::new(insert, default_vi_normal_keybindings()))
            }
            _ => {
                // Emacs mode (default)
                let mut kb = default_emacs_keybindings();

                // Tab → completion menu
                kb.add_binding(
                    KeyModifiers::NONE,
                    KeyCode::Tab,
                    ReedlineEvent::UntilFound(vec![
                        ReedlineEvent::Menu("completion_menu".to_string()),
                        ReedlineEvent::MenuNext,
                    ]),
                );
                // Ctrl+R → history search
                kb.add_binding(
                    KeyModifiers::CONTROL,
                    KeyCode::Char('r'),
                    ReedlineEvent::SearchHistory,
                );
                // Ctrl+F → accept inline hint
                kb.add_binding(
                    KeyModifiers::CONTROL,
                    KeyCode::Char('f'),
                    ReedlineEvent::HistoryHintComplete,
                );
                // Right Arrow → accept inline hint or move right
                kb.add_binding(
                    KeyModifiers::NONE,
                    KeyCode::Right,
                    ReedlineEvent::UntilFound(vec![
                        ReedlineEvent::HistoryHintComplete,
                        ReedlineEvent::Right,
                    ]),
                );
                // Ctrl+E → end of line
                kb.add_binding(
                    KeyModifiers::CONTROL,
                    KeyCode::Char('e'),
                    ReedlineEvent::Edit(vec![EditCommand::MoveToLineEnd { select: false }]),
                );
                // Alt+. → insert last word (like zsh/bash)
                kb.add_binding(
                    KeyModifiers::ALT,
                    KeyCode::Char('.'),
                    ReedlineEvent::HistoryHintWordComplete,
                );

                // Apply user custom keybindings
                apply_custom_keybindings(&mut kb, &config);

                Box::new(Emacs::new(kb))
            }
        };

        // ── Build editor ─────────────────────────────────────────────────────
        let editor = Reedline::create()
            .with_history(mem_history)
            .with_completer(completer)
            .with_highlighter(highlighter)
            .with_hinter(hinter)
            .with_menu(ReedlineMenu::EngineCompleter(completion_menu))
            .with_edit_mode(edit_mode)
            .with_ansi_colors(true);

        let renderer = PromptRenderer::new(&config);
        let executor = Executor::new(config.clone());

        Ok(Self {
            editor,
            executor,
            prompt: ZishPrompt::default(),
            renderer,
            config,
        })
    }

    pub fn run(mut self) -> Result<()> {
        self.run_startup_hooks();

        loop {
            // Build prompt strings
            let jobs_count = self.executor.jobs.read().unwrap().len();
            self.prompt.left = self.renderer.render_left(
                self.executor.last_exit,
                self.executor.last_cmd_duration_ms,
                jobs_count,
            );
            self.prompt.right = self.renderer.render_right(
                self.executor.last_exit,
                self.executor.last_cmd_duration_ms,
                jobs_count,
            );

            self.run_precmd_hooks();
            self.executor.jobs.write().unwrap().reap();

            match self.editor.read_line(&self.prompt) {
                Ok(Signal::Success(line)) => {
                    let line = line.trim();
                    if line.is_empty() { continue; }

                    self.run_preexec_hooks(line);

                    match self.executor.run_string(line) {
                        Ok(code) => {
                            crate::env::set_var("?", code.to_string());
                            self.executor.last_exit = code;
                            let _ = crate::history::HISTORY
                                .write()
                                .unwrap()
                                .add(line, code);
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            if msg.starts_with("return:") {
                                let code: i32 = msg["return:".len()..].parse().unwrap_or(0);
                                self.executor.last_exit = code;
                            } else if msg == "break" || msg == "continue" {
                                eprintln!("zish: {}: only valid inside a loop", msg);
                            } else {
                                eprintln!("zish: {}", e);
                                self.executor.last_exit = 1;
                            }
                            crate::env::set_var("?", self.executor.last_exit.to_string());
                        }
                    }
                }
                Ok(Signal::CtrlC) => {
                    self.executor.last_exit = 130;
                    crate::env::set_var("?", "130");
                    eprintln!();
                }
                Ok(Signal::CtrlD) => {
                    self.run_exit_hooks();
                    println!("exit");
                    break;
                }
                Err(e) => {
                    eprintln!("zish: readline error: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }

    fn run_startup_hooks(&mut self) {
        let hooks = self.config.hooks.on_startup.clone();
        for hook in hooks { let _ = self.executor.run_string(&hook); }
    }

    fn run_precmd_hooks(&mut self) {
        let hooks = self.config.hooks.precmd.clone();
        for hook in hooks { let _ = self.executor.run_string(&hook); }
    }

    fn run_preexec_hooks(&mut self, cmd: &str) {
        crate::env::set_var("ZISH_PREEXEC_CMD", cmd);
        let hooks = self.config.hooks.preexec.clone();
        for hook in hooks { let _ = self.executor.run_string(&hook); }
    }

    fn run_exit_hooks(&mut self) {
        let hooks = self.config.hooks.on_exit.clone();
        for hook in hooks { let _ = self.executor.run_string(&hook); }
    }
}

// ─── Prompt ────────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct ZishPrompt {
    pub left: String,
    pub right: String,
}

impl Prompt for ZishPrompt {
    fn render_prompt_left(&self) -> Cow<str> {
        Cow::Borrowed(&self.left)
    }

    fn render_prompt_right(&self) -> Cow<str> {
        Cow::Borrowed(&self.right)
    }

    fn render_prompt_indicator(&self, mode: PromptEditMode) -> Cow<str> {
        match mode {
            PromptEditMode::Vi(reedline::PromptViMode::Normal) => Cow::Borrowed("[N] "),
            PromptEditMode::Vi(reedline::PromptViMode::Insert) => Cow::Borrowed("[I] "),
            _ => Cow::Borrowed(""),
        }
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<str> {
        Cow::Borrowed("│ ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<str> {
        let indicator = match history_search.status {
            PromptHistorySearchStatus::Passing => "\x1b[32mbck-i-search\x1b[0m: ",
            PromptHistorySearchStatus::Failing => "\x1b[31mfailing-bck-i-search\x1b[0m: ",
        };
        Cow::Owned(format!("{}{}", indicator, history_search.term))
    }
}

// ─── Custom keybinding parser ──────────────────────────────────────────────────

fn apply_custom_keybindings(kb: &mut reedline::Keybindings, config: &Config) {
    for (key_str, action_str) in &config.keybindings.bindings {
        let Some(event) = parse_reedline_event(action_str) else { continue };
        let Some((modifiers, code)) = parse_key(key_str) else { continue };
        kb.add_binding(modifiers, code, event);
    }
}

fn parse_key(s: &str) -> Option<(KeyModifiers, KeyCode)> {
    let parts: Vec<&str> = s.split('+').collect();
    let mut modifiers = KeyModifiers::NONE;
    let key_part = parts.last()?;

    for part in &parts[..parts.len() - 1] {
        match part.to_lowercase().as_str() {
            "ctrl" => modifiers |= KeyModifiers::CONTROL,
            "alt"  => modifiers |= KeyModifiers::ALT,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            _ => {}
        }
    }

    let code = match *key_part {
        "Enter"     => KeyCode::Enter,
        "Tab"       => KeyCode::Tab,
        "Backspace" => KeyCode::Backspace,
        "Esc"       => KeyCode::Esc,
        "Left"      => KeyCode::Left,
        "Right"     => KeyCode::Right,
        "Up"        => KeyCode::Up,
        "Down"      => KeyCode::Down,
        "Home"      => KeyCode::Home,
        "End"       => KeyCode::End,
        "PageUp"    => KeyCode::PageUp,
        "PageDown"  => KeyCode::PageDown,
        "Delete"    => KeyCode::Delete,
        s if s.len() == 1 => KeyCode::Char(s.chars().next()?),
        _ => return None,
    };

    Some((modifiers, code))
}

fn parse_reedline_event(s: &str) -> Option<ReedlineEvent> {
    Some(match s {
        "accept-hint" | "HistoryHintComplete"     => ReedlineEvent::HistoryHintComplete,
        "accept-hint-word" | "HistoryHintWordComplete" => ReedlineEvent::HistoryHintWordComplete,
        "end-of-line"   => ReedlineEvent::Edit(vec![EditCommand::MoveToLineEnd { select: false }]),
        "start-of-line" => ReedlineEvent::Edit(vec![EditCommand::MoveToLineStart { select: false }]),
        "SearchHistory"  => ReedlineEvent::SearchHistory,
        "ClearScreen"    => ReedlineEvent::ClearScreen,
        "Enter"          => ReedlineEvent::Enter,
        "Up"             => ReedlineEvent::Up,
        "Down"           => ReedlineEvent::Down,
        "MenuNext"       => ReedlineEvent::MenuNext,
        _ => return None,
    })
}

// ─── Custom Fish-Style Hinter ──────────────────────────────────────────────────

pub struct ZishHinter {
    completer: completer::ZishCompleter,
    style: Style,
    current_hint: String,
}

impl ZishHinter {
    pub fn new(config: &Config) -> Self {
        Self {
            completer: completer::ZishCompleter::new(config),
            style: Style::new().italic().fg(Color::DarkGray),
            current_hint: String::new(),
        }
    }
}

impl reedline::Hinter for ZishHinter {
    fn handle(
        &mut self,
        line: &str,
        pos: usize,
        history: &dyn reedline::History,
        use_ansi_coloring: bool,
        _cwd: &str,
    ) -> String {
        self.current_hint = String::new();

        if line.is_empty() {
            return String::new();
        }

        // 1. Try matching command/argument completions first to get the immediate suggestion
        use reedline::Completer;
        let completions = self.completer.complete(line, pos);
        let mut completion_hint = String::new();
        if let Some(first_completion) = completions.first() {
            let last_word = line.split_whitespace().last().unwrap_or("");
            if !last_word.is_empty() && first_completion.value.starts_with(last_word) {
                let remainder = &first_completion.value[last_word.len()..];
                completion_hint = remainder.to_string();
            }
        }

        if !completion_hint.is_empty() {
            self.current_hint = completion_hint;
        } else {
            // 2. Fall back to history prefix search
            let history_hint = history
                .search(reedline::SearchQuery::last_with_prefix(
                    line.to_string(),
                    history.session(),
                ))
                .ok()
                .and_then(|entries| entries.first().cloned())
                .map(|entry| {
                    entry
                        .command_line
                        .get(line.len()..)
                        .unwrap_or_default()
                        .to_string()
                })
                .unwrap_or_default();

            if !history_hint.is_empty() {
                self.current_hint = history_hint;
            }
        }

        if use_ansi_coloring && !self.current_hint.is_empty() {
            self.style.paint(&self.current_hint).to_string()
        } else {
            self.current_hint.clone()
        }
    }

    fn complete_hint(&self) -> String {
        self.current_hint.clone()
    }

    fn next_hint_token(&self) -> String {
        self.current_hint
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string()
    }
}
