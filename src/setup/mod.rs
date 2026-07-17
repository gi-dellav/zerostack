use std::collections::HashMap;
use std::io::{self, Write};

use compact_str::CompactString;
use crossterm::ExecutableCommand;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType};

use crate::config::{Config, CustomProviderConfig, QuickModelConfig};

#[derive(Debug, Clone)]
pub enum SetupOutcome {
    Launch,
    LaunchAutoconfigure,
    Quit,
}

#[derive(Clone, Debug)]
struct TextInput {
    buffer: String,
    cursor: usize,
    original: String,
}

impl TextInput {
    fn new(text: String) -> Self {
        TextInput {
            cursor: text.chars().count(),
            original: text.clone(),
            buffer: text,
        }
    }

    fn insert(&mut self, c: char) {
        let byte_pos = self
            .buffer
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.buffer.len());
        self.buffer.insert(byte_pos, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let byte_pos = self
                .buffer
                .char_indices()
                .nth(self.cursor)
                .map(|(i, _)| i)
                .unwrap_or(self.buffer.len());
            self.buffer.remove(byte_pos);
        }
    }

    fn delete(&mut self) {
        let byte_pos = self
            .buffer
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.buffer.len());
        if byte_pos < self.buffer.len() {
            self.buffer.remove(byte_pos);
        }
    }

    fn cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn cursor_right(&mut self) {
        if self.cursor < self.buffer.chars().count() {
            self.cursor += 1;
        }
    }

    fn cursor_home(&mut self) {
        self.cursor = 0;
    }

    fn cursor_end(&mut self) {
        self.cursor = self.buffer.chars().count();
    }

    fn confirmed(&self) -> String {
        self.buffer.clone()
    }
}

#[derive(Clone, Debug)]
struct FieldDef {
    label: &'static str,
    value: String,
    editable: bool,
    masked: bool,
}

impl FieldDef {
    fn owned_clone(&self) -> FieldDef {
        FieldDef {
            label: self.label,
            value: self.value.clone(),
            editable: self.editable,
            masked: self.masked,
        }
    }
}

fn clone_fields(fields: &[FieldDef]) -> Vec<FieldDef> {
    fields.iter().map(|f| f.owned_clone()).collect()
}

#[derive(Clone, Debug)]
enum Screen {
    MainMenu,
    ManageProviders {
        selected: usize,
        confirm_delete: bool,
    },
    ProviderDetail {
        is_new: bool,
        is_builtin: bool,
        name: String,
        fields: Vec<FieldDef>,
        selected_field: usize,
        editing: Option<TextInput>,
        error: Option<String>,
    },
    ManageModels {
        selected: usize,
        confirm_delete: bool,
    },
    ModelDetail {
        is_new: bool,
        name: String,
        fields: Vec<FieldDef>,
        selected_field: usize,
        editing: Option<TextInput>,
        error: Option<String>,
    },
}

struct Ctx {
    cfg: Config,
    screen: Screen,
    cols: u16,
    rows: u16,
    message: Option<String>,
}

fn builtin_provider_names() -> &'static [&'static str] {
    &["openrouter", "openai", "anthropic", "gemini", "ollama"]
}

fn provider_env_var(name: &str) -> &'static str {
    match name {
        "openrouter" => "OPENROUTER_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "anthropic" => "ANTHROPIC_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        "ollama" => "OLLAMA_API_KEY",
        _ => "",
    }
}

fn collect_provider_names(cfg: &Config) -> Vec<String> {
    let mut names: Vec<String> = builtin_provider_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some(custom) = &cfg.custom_providers {
        for name in custom.keys() {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
    }
    names
}

fn collect_models(cfg: &Config) -> Vec<(String, QuickModelConfig)> {
    let mut models: Vec<(String, QuickModelConfig)> = cfg
        .quick_models
        .as_ref()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    models.sort_by(|a, b| a.0.cmp(&b.0));
    models
}

fn has_env(env_name: &str) -> bool {
    if env_name.is_empty() {
        return false;
    }
    std::env::var(env_name)
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

fn masked_key(key: &str) -> String {
    if key.is_empty() {
        return "(not set)".to_string();
    }
    if key.len() <= 4 {
        return "****".to_string();
    }
    format!("{}****", &key[..4.min(key.len())])
}

fn provider_type_display(name: &str, cfg: &Config) -> String {
    if builtin_provider_names().contains(&name) {
        format!("[built-in: {}]", name)
    } else if let Some(custom) = cfg.custom_providers.as_ref().and_then(|m| m.get(name)) {
        format!("[custom: {}]", custom.provider_type)
    } else {
        "[unknown]".to_string()
    }
}

fn clear_screen() -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.execute(Hide)?;
    stdout.execute(Clear(ClearType::All))?;
    stdout.execute(MoveTo(0, 0))?;
    Ok(())
}

fn write_centered(row: u16, text: &str, color: Color) -> io::Result<()> {
    let mut stdout = io::stdout();
    let (cols, _) = terminal::size()?;
    let x = if cols as usize > text.len() {
        (cols as usize - text.len()) / 2
    } else {
        0
    };
    stdout.execute(MoveTo(x as u16, row))?;
    stdout.execute(SetForegroundColor(color))?;
    stdout.execute(Print(text))?;
    stdout.execute(ResetColor)?;
    Ok(())
}

fn write_line(row: u16, col: u16, text: &str, color: Color) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.execute(MoveTo(col, row))?;
    stdout.execute(SetForegroundColor(color))?;
    stdout.execute(Print(text))?;
    stdout.execute(ResetColor)?;
    Ok(())
}

fn write_hline(row: u16, color: Color) -> io::Result<()> {
    let mut stdout = io::stdout();
    let (cols, _) = terminal::size()?;
    stdout.execute(MoveTo(0, row))?;
    stdout.execute(SetForegroundColor(color))?;
    let line = "\u{2500}".repeat(cols as usize);
    stdout.execute(Print(&line))?;
    stdout.execute(ResetColor)?;
    Ok(())
}

fn stdout_flush() -> io::Result<()> {
    let (_, rows) = terminal::size()?;
    let mut stdout = io::stdout();
    stdout.execute(MoveTo(0, rows.saturating_sub(1)))?;
    stdout.execute(Show)?;
    stdout.flush()?;
    Ok(())
}

fn render(ctx: &Ctx) -> io::Result<()> {
    clear_screen()?;
    match &ctx.screen {
        Screen::MainMenu => render_main_menu(ctx),
        Screen::ManageProviders {
            selected,
            confirm_delete,
        } => render_manage_providers(ctx, *selected, *confirm_delete),
        Screen::ProviderDetail { .. } => render_provider_detail(ctx),
        Screen::ManageModels {
            selected,
            confirm_delete,
        } => render_manage_models(ctx, *selected, *confirm_delete),
        Screen::ModelDetail { .. } => render_model_detail(ctx),
    }
}

fn render_main_menu(ctx: &Ctx) -> io::Result<()> {
    write_centered(0, "ZEROSTACK SETUP", Color::Cyan)?;
    write_hline(2, Color::DarkGrey)?;

    let mut row = 4u16;

    write_line(row, 2, "PROVIDERS", Color::Yellow)?;
    row += 1;
    let providers = collect_provider_names(&ctx.cfg);
    if providers.is_empty() {
        write_line(row, 4, "(none)", Color::DarkGrey)?;
        row += 1;
    } else {
        for name in &providers {
            let type_str = provider_type_display(name, &ctx.cfg);
            write_line(row, 4, name, Color::White)?;
            write_line(row, 4 + name.len() as u16 + 2, &type_str, Color::DarkGrey)?;
            row += 1;
        }
    }

    row += 1;
    write_line(row, 2, "QUICK MODELS", Color::Yellow)?;
    row += 1;
    let models = collect_models(&ctx.cfg);
    if models.is_empty() {
        write_line(row, 4, "(none)", Color::DarkGrey)?;
        row += 1;
    } else {
        for (name, qm) in &models {
            let line = format!("{}  \u{2192}  {} / {}", name, qm.provider, qm.model);
            write_line(row, 4, &line, Color::White)?;
            row += 1;
        }
    }

    write_hline(row + 1, Color::DarkGrey)?;
    row += 3;

    write_line(row, 2, "P) Manage Providers", Color::White)?;
    write_line(row, 28, "M) Manage Models", Color::White)?;
    row += 1;
    write_line(row, 2, "L) Launch agent", Color::White)?;
    write_line(row, 28, "A) Autoconfigure", Color::White)?;
    row += 1;
    write_line(row, 2, "Q) Quit", Color::White)?;

    if let Some(msg) = &ctx.message {
        write_line(row + 2, 2, msg, Color::DarkYellow)?;
    }

    stdout_flush()?;
    Ok(())
}

fn render_manage_providers(ctx: &Ctx, selected: usize, confirm_delete: bool) -> io::Result<()> {
    write_centered(0, "MANAGE PROVIDERS", Color::Cyan)?;
    write_hline(2, Color::DarkGrey)?;

    let mut row = 4u16;
    let providers = collect_provider_names(&ctx.cfg);

    if providers.is_empty() {
        write_line(row, 4, "(no providers configured)", Color::DarkGrey)?;
        row += 2;
    } else {
        for (i, name) in providers.iter().enumerate() {
            let highlight = i == selected;
            let color = if highlight { Color::Cyan } else { Color::White };
            let marker = if highlight { ">" } else { " " };
            let is_builtin = builtin_provider_names().contains(&name.as_str());
            let type_str = if is_builtin {
                "[built-in]".to_string()
            } else if let Some(c) = ctx.cfg.custom_providers.as_ref().and_then(|m| m.get(name)) {
                format!("[custom: {}]  {}", c.provider_type, c.base_url)
            } else {
                "[unknown]".to_string()
            };
            let env = provider_env_var(name);
            let cfg_key = ctx
                .cfg
                .api_keys
                .as_ref()
                .and_then(|k| k.get(name))
                .filter(|v| !v.is_empty());
            let key_str = if cfg_key.is_some() || has_env(env) {
                "key: set"
            } else {
                "key: not set"
            };
            write_line(
                row,
                4,
                &format!(
                    "{} {}. {}    {}    {}",
                    marker,
                    i + 1,
                    name,
                    type_str,
                    key_str
                ),
                color,
            )?;
            row += 1;
        }
    }

    write_hline(row + 1, Color::DarkGrey)?;
    row += 3;

    if confirm_delete {
        let selected_name = providers.get(selected).map(|s| s.as_str()).unwrap_or("");
        write_line(
            row,
            4,
            &format!(
                "Delete '{}'? This cannot be undone. [Y] Yes  [N] No",
                selected_name
            ),
            Color::Red,
        )?;
    } else {
        write_line(row, 4, "A) Add custom provider", Color::White)?;
        write_line(row, 32, "Enter) View / Edit selected", Color::White)?;
        row += 1;
        let del_label = if let Some(name) = providers.get(selected) {
            if builtin_provider_names().contains(&name.as_str()) {
                "D) (cannot delete built-in provider)"
            } else {
                "D) Delete selected"
            }
        } else {
            "D) Delete selected"
        };
        write_line(row, 4, del_label, Color::White)?;
        write_line(row, 32, "Esc) Back", Color::White)?;
    }

    if let Some(msg) = &ctx.message {
        write_line(row + 2, 4, msg, Color::DarkYellow)?;
    }

    stdout_flush()?;
    Ok(())
}

fn render_provider_detail(ctx: &Ctx) -> io::Result<()> {
    let (is_new, name, fields, selected_field, editing, error) = match &ctx.screen {
        Screen::ProviderDetail {
            is_new,
            name,
            fields,
            selected_field,
            editing,
            error,
            ..
        } => (*is_new, name, fields, *selected_field, editing, error),
        _ => return Ok(()),
    };

    let title = if is_new {
        "ADD PROVIDER".to_string()
    } else {
        format!("EDIT PROVIDER: {}", name)
    };
    write_centered(0, &title, Color::Cyan)?;
    write_hline(2, Color::DarkGrey)?;

    let mut row = 4u16;
    for (i, field) in fields.iter().enumerate() {
        let highlight = i == selected_field;
        let color = if highlight && editing.is_none() {
            Color::Cyan
        } else {
            Color::White
        };
        let marker = if highlight && editing.is_none() {
            ">"
        } else {
            " "
        };

        let value_display = if let Some(ed) = editing
            && i == selected_field
        {
            if field.masked {
                let visible = if ed.buffer.is_empty() {
                    "(empty)"
                } else {
                    &ed.buffer
                };
                format!("{} (editing...)", "*".repeat(visible.len().min(16)))
            } else {
                ed.buffer.clone()
            }
        } else if field.masked && !field.value.is_empty() {
            masked_key(&field.value)
        } else if field.value.is_empty() {
            "(not set)".to_string()
        } else {
            field.value.clone()
        };

        let editable_marker = if field.editable { "" } else { " (read-only)" };
        write_line(
            row,
            4,
            &format!(
                "{} {:20}  {}{}",
                marker, field.label, value_display, editable_marker
            ),
            color,
        )?;
        row += 1;
    }

    write_hline(row + 1, Color::DarkGrey)?;
    row += 3;

    if editing.is_some() {
        write_line(
            row,
            4,
            "[Enter] Confirm  [Esc] Cancel  [Tab] Toggle mask",
            Color::White,
        )?;
    } else {
        write_line(
            row,
            4,
            "[Up/Down] Select field  [Enter] Edit  [S] Save  [Esc] Back",
            Color::White,
        )?;
    }

    if let Some(err) = error {
        write_line(row + 2, 4, err, Color::Red)?;
    }

    stdout_flush()?;
    Ok(())
}

fn render_manage_models(ctx: &Ctx, selected: usize, confirm_delete: bool) -> io::Result<()> {
    write_centered(0, "MANAGE MODELS", Color::Cyan)?;
    write_hline(2, Color::DarkGrey)?;

    let mut row = 4u16;
    let models = collect_models(&ctx.cfg);

    if models.is_empty() {
        write_line(row, 4, "(no quick models defined)", Color::DarkGrey)?;
        row += 2;
    } else {
        for (i, (name, qm)) in models.iter().enumerate() {
            let highlight = i == selected;
            let color = if highlight { Color::Cyan } else { Color::White };
            let marker = if highlight { ">" } else { " " };
            write_line(
                row,
                4,
                &format!(
                    "{} {}. {}    {} / {}    ${:.4}/M in  ${:.4}/M out",
                    marker,
                    i + 1,
                    name,
                    qm.provider,
                    qm.model,
                    qm.input_token_cost,
                    qm.output_token_cost
                ),
                color,
            )?;
            row += 1;
        }
    }

    write_hline(row + 1, Color::DarkGrey)?;
    row += 3;

    if confirm_delete {
        let selected_name = models.get(selected).map(|(n, _)| n.as_str()).unwrap_or("");
        write_line(
            row,
            4,
            &format!(
                "Delete '{}'? This cannot be undone. [Y] Yes  [N] No",
                selected_name
            ),
            Color::Red,
        )?;
    } else {
        write_line(row, 4, "A) Add model", Color::White)?;
        write_line(row, 32, "Enter) View / Edit selected", Color::White)?;
        row += 1;
        write_line(row, 4, "D) Delete selected", Color::White)?;
        write_line(row, 32, "Esc) Back", Color::White)?;
    }

    if let Some(msg) = &ctx.message {
        write_line(row + 2, 4, msg, Color::DarkYellow)?;
    }

    stdout_flush()?;
    Ok(())
}

fn render_model_detail(ctx: &Ctx) -> io::Result<()> {
    let (is_new, name, fields, selected_field, editing, error) = match &ctx.screen {
        Screen::ModelDetail {
            is_new,
            name,
            fields,
            selected_field,
            editing,
            error,
        } => (*is_new, name, fields, *selected_field, editing, error),
        _ => return Ok(()),
    };

    let title = if is_new {
        "ADD MODEL".to_string()
    } else {
        format!("EDIT MODEL: {}", name)
    };
    write_centered(0, &title, Color::Cyan)?;
    write_hline(2, Color::DarkGrey)?;

    let mut row = 4u16;
    for (i, field) in fields.iter().enumerate() {
        let highlight = i == selected_field;
        let color = if highlight && editing.is_none() {
            Color::Cyan
        } else {
            Color::White
        };
        let marker = if highlight && editing.is_none() {
            ">"
        } else {
            " "
        };

        let value_display = if let Some(ed) = editing
            && i == selected_field
        {
            ed.buffer.clone()
        } else if field.value.is_empty() {
            "(not set)".to_string()
        } else {
            field.value.clone()
        };

        let editable_marker = if field.editable { "" } else { " (read-only)" };
        write_line(
            row,
            4,
            &format!(
                "{} {:20}  {}{}",
                marker, field.label, value_display, editable_marker
            ),
            color,
        )?;
        row += 1;
    }

    write_hline(row + 1, Color::DarkGrey)?;
    row += 3;

    if editing.is_some() {
        write_line(row, 4, "[Enter] Confirm  [Esc] Cancel", Color::White)?;
    } else {
        write_line(
            row,
            4,
            "[Up/Down] Select field  [Enter] Edit  [S] Save  [Esc] Back",
            Color::White,
        )?;
    }

    if let Some(err) = error {
        write_line(row + 2, 4, err, Color::Red)?;
    }

    stdout_flush()?;
    Ok(())
}

fn make_provider_detail_state(
    is_new: bool,
    is_builtin: bool,
    name: String,
    cfg: &Config,
) -> Screen {
    let provider_type = if is_builtin {
        name.clone()
    } else if let Some(c) = cfg.custom_providers.as_ref().and_then(|m| m.get(&name)) {
        c.provider_type.to_string()
    } else {
        String::new()
    };

    let base_url = cfg
        .custom_providers
        .as_ref()
        .and_then(|m| m.get(&name))
        .map(|c| c.base_url.clone())
        .unwrap_or_default();

    let api_key_env = if is_builtin {
        provider_env_var(&name).to_string()
    } else {
        cfg.custom_providers
            .as_ref()
            .and_then(|m| m.get(&name))
            .and_then(|c| c.api_key_env.as_ref().map(|s| s.to_string()))
            .unwrap_or_default()
    };

    let api_key_value = cfg
        .api_keys
        .as_ref()
        .and_then(|k| k.get(&name))
        .cloned()
        .unwrap_or_default();

    let env_key_value = if !api_key_env.is_empty() {
        let env_val = std::env::var(&api_key_env).unwrap_or_default();
        if !env_val.is_empty() {
            env_val
        } else {
            api_key_value.clone()
        }
    } else {
        api_key_value.clone()
    };

    let mut fields: Vec<FieldDef> = Vec::new();

    if !is_builtin {
        fields.push(FieldDef {
            label: "Name",
            value: name.clone(),
            editable: is_new,
            masked: false,
        });
        fields.push(FieldDef {
            label: "Provider Type",
            value: provider_type,
            editable: true,
            masked: false,
        });
        fields.push(FieldDef {
            label: "Base URL",
            value: base_url,
            editable: true,
            masked: false,
        });
    }

    fields.push(FieldDef {
        label: if is_builtin {
            "API Key Env"
        } else {
            "API Key Env Var"
        },
        value: api_key_env,
        editable: !is_builtin,
        masked: false,
    });

    fields.push(FieldDef {
        label: "API Key Value",
        value: env_key_value,
        editable: true,
        masked: true,
    });

    Screen::ProviderDetail {
        is_new,
        is_builtin,
        name,
        fields,
        selected_field: 0,
        editing: None,
        error: None,
    }
}

pub fn run(cfg: &mut Config) -> anyhow::Result<SetupOutcome> {
    terminal::enable_raw_mode()?;
    let result = run_inner(cfg);
    let _ = terminal::disable_raw_mode();
    let mut stdout = io::stdout();
    let _ = stdout.execute(Clear(ClearType::All));
    let _ = stdout.execute(MoveTo(0, 0));
    let _ = stdout.execute(Show);
    let _ = stdout.flush();
    result
}

fn run_inner(cfg: &mut Config) -> anyhow::Result<SetupOutcome> {
    let (cols, rows) = terminal::size()?;
    let mut ctx = Ctx {
        cfg: cfg.clone(),
        screen: Screen::MainMenu,
        cols,
        rows,
        message: None,
    };

    loop {
        if let Err(e) = render(&ctx) {
            tracing::warn!("render error: {e}");
        }

        let event = crossterm::event::read()?;
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                let outcome = handle_key(&ctx, key)?;
                match outcome {
                    KeyResult::Screen(new_screen) => {
                        ctx.screen = new_screen;
                        ctx.message = None;
                    }
                    KeyResult::Outcome(outcome, new_cfg) => {
                        *cfg = new_cfg.clone();
                        crate::config::save_config(cfg)?;
                        return Ok(outcome);
                    }
                }
            }
            Event::Resize(c, r) => {
                ctx.cols = c;
                ctx.rows = r;
            }
            _ => {}
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum KeyResult {
    Screen(Screen),
    Outcome(SetupOutcome, Config),
}

fn handle_key(ctx: &Ctx, key: KeyEvent) -> anyhow::Result<KeyResult> {
    match &ctx.screen {
        Screen::MainMenu => handle_main_menu_key(ctx, key),
        Screen::ManageProviders {
            selected,
            confirm_delete,
        } => handle_manage_providers_key(ctx, *selected, *confirm_delete, key),
        Screen::ProviderDetail { .. } => handle_provider_detail_key(ctx, key),
        Screen::ManageModels {
            selected,
            confirm_delete,
        } => handle_manage_models_key(ctx, *selected, *confirm_delete, key),
        Screen::ModelDetail { .. } => handle_model_detail_key(ctx, key),
    }
}

fn handle_main_menu_key(ctx: &Ctx, key: KeyEvent) -> anyhow::Result<KeyResult> {
    match key.code {
        KeyCode::Char('p') | KeyCode::Char('P') => Ok(KeyResult::Screen(Screen::ManageProviders {
            selected: 0,
            confirm_delete: false,
        })),
        KeyCode::Char('m') | KeyCode::Char('M') => Ok(KeyResult::Screen(Screen::ManageModels {
            selected: 0,
            confirm_delete: false,
        })),
        KeyCode::Char('l') | KeyCode::Char('L') => {
            Ok(KeyResult::Outcome(SetupOutcome::Launch, ctx.cfg.clone()))
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            let mut new_cfg = ctx.cfg.clone();
            apply_autoconfigure(&mut new_cfg);
            Ok(KeyResult::Outcome(
                SetupOutcome::LaunchAutoconfigure,
                new_cfg,
            ))
        }
        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
            Ok(KeyResult::Outcome(SetupOutcome::Quit, ctx.cfg.clone()))
        }
        _ => Ok(KeyResult::Screen(ctx.screen.clone())),
    }
}

fn handle_manage_providers_key(
    ctx: &Ctx,
    selected: usize,
    confirm_delete: bool,
    key: KeyEvent,
) -> anyhow::Result<KeyResult> {
    let providers = collect_provider_names(&ctx.cfg);
    let count = providers.len();

    if confirm_delete {
        return match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let mut new_cfg = ctx.cfg.clone();
                if let Some(name) = providers.get(selected) {
                    let is_builtin = builtin_provider_names().contains(&name.as_str());
                    if !is_builtin {
                        if let Some(m) = new_cfg.custom_providers.as_mut() {
                            m.remove(name);
                        }
                        if let Some(m) = new_cfg.api_keys.as_mut() {
                            m.remove(name);
                        }
                    }
                }
                let new_selected = selected.min(count.saturating_sub(2));
                Ok(KeyResult::Screen(Screen::ManageProviders {
                    selected: new_selected,
                    confirm_delete: false,
                }))
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                Ok(KeyResult::Screen(Screen::ManageProviders {
                    selected,
                    confirm_delete: false,
                }))
            }
            _ => Ok(KeyResult::Screen(ctx.screen.clone())),
        };
    }

    match key.code {
        KeyCode::Up => {
            let s = if selected == 0 {
                count.saturating_sub(1)
            } else {
                selected - 1
            };
            Ok(KeyResult::Screen(Screen::ManageProviders {
                selected: s,
                confirm_delete: false,
            }))
        }
        KeyCode::Down => {
            let s = if count == 0 {
                0
            } else {
                (selected + 1).min(count - 1)
            };
            Ok(KeyResult::Screen(Screen::ManageProviders {
                selected: s,
                confirm_delete: false,
            }))
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            let new_screen = make_provider_detail_state(true, false, String::new(), &ctx.cfg);
            Ok(KeyResult::Screen(new_screen))
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if let Some(name) = providers.get(selected) {
                if builtin_provider_names().contains(&name.as_str()) {
                    Ok(KeyResult::Screen(ctx.screen.clone()))
                } else {
                    Ok(KeyResult::Screen(Screen::ManageProviders {
                        selected,
                        confirm_delete: true,
                    }))
                }
            } else {
                Ok(KeyResult::Screen(ctx.screen.clone()))
            }
        }
        KeyCode::Enter => {
            if let Some(name) = providers.get(selected).cloned() {
                let is_builtin = builtin_provider_names().contains(&name.as_str());
                let new_screen = make_provider_detail_state(false, is_builtin, name, &ctx.cfg);
                Ok(KeyResult::Screen(new_screen))
            } else {
                Ok(KeyResult::Screen(ctx.screen.clone()))
            }
        }
        KeyCode::Esc => Ok(KeyResult::Screen(Screen::MainMenu)),
        _ => Ok(KeyResult::Screen(ctx.screen.clone())),
    }
}

fn handle_provider_detail_key(ctx: &Ctx, key: KeyEvent) -> anyhow::Result<KeyResult> {
    let screen_data = match &ctx.screen {
        Screen::ProviderDetail {
            is_new,
            is_builtin,
            name,
            fields,
            selected_field,
            editing,
            error: _,
        } => Some((
            *is_new,
            *is_builtin,
            name.clone(),
            clone_fields(fields),
            *selected_field,
            editing.clone(),
        )),
        _ => None,
    };

    let (is_new, is_builtin, name, mut fields, selected_field, editing) = match screen_data {
        Some(d) => d,
        None => return Ok(KeyResult::Screen(ctx.screen.clone())),
    };

    let make_screen = |f: Vec<FieldDef>, sf: usize, ed: Option<TextInput>, err: Option<String>| {
        Screen::ProviderDetail {
            is_new,
            is_builtin,
            name: name.clone(),
            fields: f,
            selected_field: sf,
            editing: ed,
            error: err,
        }
    };

    if let Some(mut ed) = editing {
        match key.code {
            KeyCode::Enter => {
                fields[selected_field].value = ed.confirmed();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    None,
                    None,
                )))
            }
            KeyCode::Esc => {
                fields[selected_field].value = ed.original.clone();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    None,
                    None,
                )))
            }
            KeyCode::Backspace => {
                ed.backspace();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            KeyCode::Delete => {
                ed.delete();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            KeyCode::Left => {
                ed.cursor_left();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            KeyCode::Right => {
                ed.cursor_right();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            KeyCode::Home => {
                ed.cursor_home();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            KeyCode::End => {
                ed.cursor_end();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            KeyCode::Tab => {
                fields[selected_field].masked = !fields[selected_field].masked;
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            KeyCode::Char(c) => {
                ed.insert(c);
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            _ => Ok(KeyResult::Screen(ctx.screen.clone())),
        }
    } else {
        match key.code {
            KeyCode::Up => {
                let s = if selected_field == 0 {
                    fields.len().saturating_sub(1)
                } else {
                    selected_field - 1
                };
                Ok(KeyResult::Screen(make_screen(fields, s, None, None)))
            }
            KeyCode::Down => {
                let s = if fields.is_empty() {
                    0
                } else {
                    (selected_field + 1).min(fields.len() - 1)
                };
                Ok(KeyResult::Screen(make_screen(fields, s, None, None)))
            }
            KeyCode::Enter => {
                if let Some(field) = fields.get(selected_field)
                    && field.editable
                {
                    let ti = TextInput::new(field.value.clone());
                    Ok(KeyResult::Screen(make_screen(
                        fields,
                        selected_field,
                        Some(ti),
                        None,
                    )))
                } else {
                    Ok(KeyResult::Screen(ctx.screen.clone()))
                }
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                let mut new_cfg = ctx.cfg.clone();

                let new_name = fields
                    .iter()
                    .find(|f| f.label == "Name")
                    .map(|f| f.value.clone())
                    .unwrap_or_else(|| name.clone());

                let new_provider_type = fields
                    .iter()
                    .find(|f| f.label == "Provider Type")
                    .map(|f| f.value.clone())
                    .unwrap_or_default();

                let new_base_url = fields
                    .iter()
                    .find(|f| f.label == "Base URL")
                    .map(|f| f.value.clone())
                    .unwrap_or_default();

                let new_api_key_env = fields
                    .iter()
                    .find(|f| f.label == "API Key Env Var" || f.label == "API Key Env")
                    .map(|f| f.value.clone())
                    .unwrap_or_default();

                let new_api_key_value = fields
                    .iter()
                    .find(|f| f.label == "API Key Value")
                    .map(|f| f.value.clone())
                    .unwrap_or_default();

                if is_builtin {
                    if !new_api_key_value.is_empty() {
                        let keys = new_cfg.api_keys.get_or_insert_with(HashMap::new);
                        keys.insert(name.clone(), new_api_key_value);
                    } else if let Some(ref mut keys) = new_cfg.api_keys {
                        keys.remove(&name);
                    }
                    return Ok(KeyResult::Screen(Screen::MainMenu));
                }

                if is_new && new_name.is_empty() {
                    return Ok(KeyResult::Screen(make_screen(
                        fields,
                        selected_field,
                        None,
                        Some("Name is required".to_string()),
                    )));
                }
                if is_new && new_provider_type.is_empty() {
                    return Ok(KeyResult::Screen(make_screen(
                        fields,
                        selected_field,
                        None,
                        Some("Provider Type is required".to_string()),
                    )));
                }
                if is_new && new_base_url.is_empty() {
                    return Ok(KeyResult::Screen(make_screen(
                        fields,
                        selected_field,
                        None,
                        Some("Base URL is required for custom providers".to_string()),
                    )));
                }

                let final_name = if is_new {
                    new_name.clone()
                } else {
                    name.clone()
                };

                let custom = new_cfg.custom_providers.get_or_insert_with(HashMap::new);
                custom.insert(
                    final_name.clone(),
                    CustomProviderConfig {
                        provider_type: CompactString::new(&new_provider_type),
                        base_url: new_base_url,
                        api_key_env: if new_api_key_env.is_empty() {
                            None
                        } else {
                            Some(CompactString::new(&new_api_key_env))
                        },
                        danger_accept_invalid_certs: None,
                        api_style: None,
                        headers: HashMap::new(),
                        timeout_secs: None,
                        model: None,
                    },
                );

                if !new_api_key_value.is_empty() {
                    let keys = new_cfg.api_keys.get_or_insert_with(HashMap::new);
                    keys.insert(final_name.clone(), new_api_key_value);
                }

                Ok(KeyResult::Screen(Screen::MainMenu))
            }
            KeyCode::Esc => Ok(KeyResult::Screen(Screen::MainMenu)),
            _ => Ok(KeyResult::Screen(ctx.screen.clone())),
        }
    }
}

fn handle_manage_models_key(
    ctx: &Ctx,
    selected: usize,
    confirm_delete: bool,
    key: KeyEvent,
) -> anyhow::Result<KeyResult> {
    let models = collect_models(&ctx.cfg);
    let count = models.len();

    if confirm_delete {
        return match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let mut new_cfg = ctx.cfg.clone();
                if let Some((name, _)) = models.get(selected)
                    && let Some(m) = new_cfg.quick_models.as_mut()
                {
                    m.remove(name);
                }
                let new_selected = selected.min(count.saturating_sub(2));
                Ok(KeyResult::Screen(Screen::ManageModels {
                    selected: new_selected,
                    confirm_delete: false,
                }))
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                Ok(KeyResult::Screen(Screen::ManageModels {
                    selected,
                    confirm_delete: false,
                }))
            }
            _ => Ok(KeyResult::Screen(ctx.screen.clone())),
        };
    }

    match key.code {
        KeyCode::Up => {
            let s = if selected == 0 {
                count.saturating_sub(1)
            } else {
                selected - 1
            };
            Ok(KeyResult::Screen(Screen::ManageModels {
                selected: s,
                confirm_delete: false,
            }))
        }
        KeyCode::Down => {
            let s = if count == 0 {
                0
            } else {
                (selected + 1).min(count - 1)
            };
            Ok(KeyResult::Screen(Screen::ManageModels {
                selected: s,
                confirm_delete: false,
            }))
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            let new_screen = make_model_detail_state(true, String::new(), &ctx.cfg);
            Ok(KeyResult::Screen(new_screen))
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if let Some((_, _)) = models.get(selected) {
                Ok(KeyResult::Screen(Screen::ManageModels {
                    selected,
                    confirm_delete: true,
                }))
            } else {
                Ok(KeyResult::Screen(ctx.screen.clone()))
            }
        }
        KeyCode::Enter => {
            if let Some((name, _)) = models.get(selected) {
                let new_screen = make_model_detail_state(false, name.clone(), &ctx.cfg);
                Ok(KeyResult::Screen(new_screen))
            } else {
                Ok(KeyResult::Screen(ctx.screen.clone()))
            }
        }
        KeyCode::Esc => Ok(KeyResult::Screen(Screen::MainMenu)),
        _ => Ok(KeyResult::Screen(ctx.screen.clone())),
    }
}

fn make_model_detail_state(is_new: bool, name: String, cfg: &Config) -> Screen {
    let qm = if is_new {
        None
    } else {
        cfg.quick_models.as_ref().and_then(|m| m.get(&name))
    };

    let provider = qm.map(|q| q.provider.to_string()).unwrap_or_default();
    let model_id = qm.map(|q| q.model.to_string()).unwrap_or_default();
    let input_cost = qm
        .map(|q| q.input_token_cost.to_string())
        .unwrap_or_else(|| "0.0".to_string());
    let output_cost = qm
        .map(|q| q.output_token_cost.to_string())
        .unwrap_or_else(|| "0.0".to_string());
    let context_window = qm
        .and_then(|q| q.context_window)
        .map(|c| c.to_string())
        .unwrap_or_default();

    let fields = vec![
        FieldDef {
            label: "Name",
            value: name.clone(),
            editable: is_new,
            masked: false,
        },
        FieldDef {
            label: "Provider",
            value: provider,
            editable: true,
            masked: false,
        },
        FieldDef {
            label: "Model ID",
            value: model_id,
            editable: true,
            masked: false,
        },
        FieldDef {
            label: "Input Cost ($/M)",
            value: input_cost,
            editable: true,
            masked: false,
        },
        FieldDef {
            label: "Output Cost ($/M)",
            value: output_cost,
            editable: true,
            masked: false,
        },
        FieldDef {
            label: "Context Window",
            value: context_window,
            editable: true,
            masked: false,
        },
    ];

    Screen::ModelDetail {
        is_new,
        name,
        fields,
        selected_field: 0,
        editing: None,
        error: None,
    }
}

fn handle_model_detail_key(ctx: &Ctx, key: KeyEvent) -> anyhow::Result<KeyResult> {
    let screen_data = match &ctx.screen {
        Screen::ModelDetail {
            is_new,
            name,
            fields,
            selected_field,
            editing,
            error: _,
        } => Some((
            *is_new,
            name.clone(),
            clone_fields(fields),
            *selected_field,
            editing.clone(),
        )),
        _ => None,
    };

    let (is_new, name, mut fields, selected_field, editing) = match screen_data {
        Some(d) => d,
        None => return Ok(KeyResult::Screen(ctx.screen.clone())),
    };

    let make_screen = |f: Vec<FieldDef>, sf: usize, ed: Option<TextInput>, err: Option<String>| {
        Screen::ModelDetail {
            is_new,
            name: name.clone(),
            fields: f,
            selected_field: sf,
            editing: ed,
            error: err,
        }
    };

    if let Some(mut ed) = editing {
        match key.code {
            KeyCode::Enter => {
                fields[selected_field].value = ed.confirmed();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    None,
                    None,
                )))
            }
            KeyCode::Esc => {
                fields[selected_field].value = ed.original.clone();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    None,
                    None,
                )))
            }
            KeyCode::Backspace => {
                ed.backspace();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            KeyCode::Delete => {
                ed.delete();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            KeyCode::Left => {
                ed.cursor_left();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            KeyCode::Right => {
                ed.cursor_right();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            KeyCode::Home => {
                ed.cursor_home();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            KeyCode::End => {
                ed.cursor_end();
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            KeyCode::Char(c) => {
                ed.insert(c);
                Ok(KeyResult::Screen(make_screen(
                    fields,
                    selected_field,
                    Some(ed),
                    None,
                )))
            }
            _ => Ok(KeyResult::Screen(ctx.screen.clone())),
        }
    } else {
        match key.code {
            KeyCode::Up => {
                let s = if selected_field == 0 {
                    fields.len().saturating_sub(1)
                } else {
                    selected_field - 1
                };
                Ok(KeyResult::Screen(make_screen(fields, s, None, None)))
            }
            KeyCode::Down => {
                let s = if fields.is_empty() {
                    0
                } else {
                    (selected_field + 1).min(fields.len() - 1)
                };
                Ok(KeyResult::Screen(make_screen(fields, s, None, None)))
            }
            KeyCode::Enter => {
                if let Some(field) = fields.get(selected_field)
                    && field.editable
                {
                    let ti = TextInput::new(field.value.clone());
                    Ok(KeyResult::Screen(make_screen(
                        fields,
                        selected_field,
                        Some(ti),
                        None,
                    )))
                } else {
                    Ok(KeyResult::Screen(ctx.screen.clone()))
                }
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                let new_name = fields
                    .iter()
                    .find(|f| f.label == "Name")
                    .map(|f| f.value.clone())
                    .unwrap_or_default();

                let new_provider = fields
                    .iter()
                    .find(|f| f.label == "Provider")
                    .map(|f| f.value.clone())
                    .unwrap_or_default();

                let new_model_id = fields
                    .iter()
                    .find(|f| f.label == "Model ID")
                    .map(|f| f.value.clone())
                    .unwrap_or_default();

                let new_input_cost = fields
                    .iter()
                    .find(|f| f.label == "Input Cost ($/M)")
                    .and_then(|f| f.value.parse::<f64>().ok())
                    .unwrap_or(0.0);

                let new_output_cost = fields
                    .iter()
                    .find(|f| f.label == "Output Cost ($/M)")
                    .and_then(|f| f.value.parse::<f64>().ok())
                    .unwrap_or(0.0);

                let new_context_window = fields
                    .iter()
                    .find(|f| f.label == "Context Window")
                    .and_then(|f| {
                        let v = f.value.trim();
                        if v.is_empty() {
                            None
                        } else {
                            v.parse::<u64>().ok()
                        }
                    });

                if new_name.is_empty() {
                    return Ok(KeyResult::Screen(make_screen(
                        fields,
                        selected_field,
                        None,
                        Some("Name is required".to_string()),
                    )));
                }
                if new_provider.is_empty() {
                    return Ok(KeyResult::Screen(make_screen(
                        fields,
                        selected_field,
                        None,
                        Some("Provider is required".to_string()),
                    )));
                }
                if new_model_id.is_empty() {
                    return Ok(KeyResult::Screen(make_screen(
                        fields,
                        selected_field,
                        None,
                        Some("Model ID is required".to_string()),
                    )));
                }

                let mut new_cfg = ctx.cfg.clone();
                let qm = new_cfg.quick_models.get_or_insert_with(HashMap::new);
                qm.insert(
                    new_name.clone(),
                    QuickModelConfig {
                        provider: CompactString::new(&new_provider),
                        model: CompactString::new(&new_model_id),
                        input_token_cost: new_input_cost,
                        output_token_cost: new_output_cost,
                        reserve_tokens: None,
                        temperature: None,
                        extra_body: None,
                        context_window: new_context_window,
                    },
                );

                if is_new && name != new_name {
                    Ok(KeyResult::Screen(Screen::ManageModels {
                        selected: 0,
                        confirm_delete: false,
                    }))
                } else {
                    Ok(KeyResult::Screen(Screen::MainMenu))
                }
            }
            KeyCode::Esc => Ok(KeyResult::Screen(Screen::MainMenu)),
            _ => Ok(KeyResult::Screen(ctx.screen.clone())),
        }
    }
}

fn apply_autoconfigure(cfg: &mut Config) {
    let providers_to_check: &[(&str, &str)] = &[
        ("openai", "OPENAI_API_KEY"),
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("gemini", "GEMINI_API_KEY"),
        ("openrouter", "OPENROUTER_API_KEY"),
    ];

    for (provider, env_var) in providers_to_check {
        if let Ok(val) = std::env::var(env_var)
            && !val.is_empty()
        {
            let keys = cfg.api_keys.get_or_insert_with(HashMap::new);
            if !keys.contains_key(*provider) {
                keys.insert(provider.to_string(), val.clone());
            }
        }
    }
}
