use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, HighlightSpacing, List, ListItem, ListState, Paragraph};
use ratatui::Terminal;

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub path: PathBuf,
    pub palette: [Color; 16],
    pub background: Color,
    pub foreground: Color,
    pub is_dark: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThemeFilter {
    All,
    Dark,
    Light,
}

impl ThemeFilter {
    fn next(self) -> Self {
        match self {
            ThemeFilter::All => ThemeFilter::Dark,
            ThemeFilter::Dark => ThemeFilter::Light,
            ThemeFilter::Light => ThemeFilter::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            ThemeFilter::All => "all",
            ThemeFilter::Dark => "dark",
            ThemeFilter::Light => "light",
        }
    }
}

fn parse_color(hex: &str) -> Option<Color> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

fn color_luminance(color: Color) -> f32 {
    let (r, g, b) = match color {
        Color::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
        Color::Black => (0.0, 0.0, 0.0),
        Color::Red => (255.0, 0.0, 0.0),
        Color::Green => (0.0, 255.0, 0.0),
        Color::Yellow => (255.0, 255.0, 0.0),
        Color::Blue => (0.0, 0.0, 255.0),
        Color::Magenta => (255.0, 0.0, 255.0),
        Color::Cyan => (0.0, 255.0, 255.0),
        Color::White => (255.0, 255.0, 255.0),
        _ => (128.0, 128.0, 128.0),
    };
    let rf = r / 255.0;
    let gf = g / 255.0;
    let bf = b / 255.0;
    0.2126 * rf + 0.7152 * gf + 0.0722 * bf
}

fn parse_theme_file(path: &Path) -> Option<Theme> {
    let content = fs::read_to_string(path).ok()?;
    let mut palette: [Color; 16] = [Color::Black; 16];
    let mut background = Color::Black;
    let mut foreground = Color::White;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(eq_pos) = trimmed.find('=') {
            let key = trimmed[..eq_pos].trim();
            let value = trimmed[eq_pos + 1..].trim();
            match key {
                "palette" => {
                    if let Some(eq_pos2) = value.find('=') {
                        let idx_str = &value[..eq_pos2];
                        let color_hex = &value[eq_pos2 + 1..];
                        if let Ok(idx) = idx_str.trim().parse::<usize>() {
                            if idx < 16 {
                                palette[idx] = parse_color(color_hex)?;
                            }
                        }
                    }
                }
                "background" => {
                    background = parse_color(value)?;
                }
                "foreground" => {
                    foreground = parse_color(value)?;
                }
                _ => {}
            }
        }
    }

    let is_dark = color_luminance(background) < 0.5;

    Some(Theme {
        name: path.file_stem()?.to_string_lossy().to_string(),
        path: path.to_path_buf(),
        palette,
        background,
        foreground,
        is_dark,
    })
}

fn discover_themes() -> Vec<Theme> {
    let mut themes = Vec::new();

    let bundled_paths = [
        "/Applications/Ghostty.app/Contents/Resources/ghostty/themes",
        "/opt/homebrew/share/ghostty/themes",
        "/usr/share/ghostty/themes",
    ];

    for dir_path in &bundled_paths {
        let dir = PathBuf::from(dir_path);
        if dir.exists() {
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(theme) = parse_theme_file(&path) {
                            themes.push(theme);
                        }
                    }
                }
            }
        }
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let user_dir = PathBuf::from(&home).join(".config/ghostty/themes");
    if user_dir.exists() {
        if let Ok(entries) = fs::read_dir(&user_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(theme) = parse_theme_file(&path) {
                        themes.push(theme);
                    }
                }
            }
        }
    }

    themes.sort_by_key(|a| a.name.to_lowercase());
    themes
}

fn apply_theme(name: &str) -> Result<(), color_eyre::Report> {
    let home = std::env::var("HOME")?;
    let auto_dir = PathBuf::from(&home).join(".config/ghostty/auto");
    fs::create_dir_all(&auto_dir)?;

    let theme_file = auto_dir.join("theme.ghostty");
    fs::write(&theme_file, format!("theme = {}\n", name))?;

    Ok(())
}

fn filter_themes(themes: &[Theme], filter: ThemeFilter, search: &str) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..themes.len()).collect();
    indices.retain(|&i| {
        let matches_filter = match filter {
            ThemeFilter::All => true,
            ThemeFilter::Dark => themes[i].is_dark,
            ThemeFilter::Light => !themes[i].is_dark,
        };
        let matches_search = if search.is_empty() {
            true
        } else {
            themes[i]
                .name
                .to_lowercase()
                .contains(&search.to_lowercase())
        };
        matches_filter && matches_search
    });
    indices
}

fn format_hex(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{:02x}{:02x}{:02x}", r, g, b),
        Color::Black => "#000000".to_string(),
        Color::Red => "#ff0000".to_string(),
        Color::Green => "#00ff00".to_string(),
        Color::Yellow => "#ffff00".to_string(),
        Color::Blue => "#0000ff".to_string(),
        Color::Magenta => "#ff00ff".to_string(),
        Color::Cyan => "#00ffff".to_string(),
        Color::White => "#ffffff".to_string(),
        _ => "unknown".to_string(),
    }
}

fn render_preview(frame: &mut ratatui::Frame, theme: &Theme, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .split(area);

    frame.render_widget(Paragraph::new(theme.name.as_str()).bold(), chunks[0]);

    let indicator = if theme.is_dark {
        "Dark Theme"
    } else {
        "Light Theme"
    };
    frame.render_widget(Paragraph::new(indicator).dim(), chunks[1]);

    frame.render_widget(Paragraph::new("System Palette").bold(), chunks[3]);

    for row in 0..2 {
        let mut spans = Vec::new();
        for col in 0..8 {
            let idx = row * 8 + col;
            let color = theme.palette[idx];

            spans.push(Span::styled(
                format!(" {idx:02} "),
                Style::default().bg(color).fg(Color::Black).bold(),
            ));
            spans.push(Span::from(" "));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), chunks[4 + row]);
    }

    let meta_lines = vec![
        Line::from(vec![
            Span::from("Background: ").bold(),
            Span::styled(" █ ", Style::default().fg(theme.background)),
            Span::from(format!("({})", format_hex(theme.background))),
        ]),
        Line::from(vec![
            Span::from("Foreground: ").bold(),
            Span::styled(" █ ", Style::default().fg(theme.foreground)),
            Span::from(format!("({})", format_hex(theme.foreground))),
        ]),
    ];
    frame.render_widget(Paragraph::new(meta_lines), chunks[7]);

    let fallback = |base: Color, light: Color| -> Color {
        if base != Color::Black {
            base
        } else {
            light
        }
    };
    let kw = Style::default().fg(fallback(theme.palette[1], theme.palette[9]));
    let fn_name = Style::default().fg(fallback(theme.palette[4], theme.palette[12]));
    let type_st = Style::default().fg(fallback(theme.palette[3], theme.palette[11]));
    let text = Style::default().fg(theme.foreground);
    let literal = Style::default().fg(fallback(theme.palette[2], theme.palette[10]));
    let macro_st = Style::default().fg(fallback(theme.palette[5], theme.palette[13]));
    let comment = Style::default()
        .fg(theme.palette[8])
        .add_modifier(Modifier::ITALIC);
    let num_col = Style::default().fg(theme.palette[8]);

    let rust_code = vec![
        Line::from(vec![
            Span::styled(" 1 │ ", num_col),
            Span::styled("// Micro inference loop check", comment),
        ]),
        Line::from(vec![
            Span::styled(" 2 │ ", num_col),
            Span::styled("pub fn ", kw),
            Span::styled("verify_health", fn_name),
            Span::styled("(", text),
            Span::styled("port", text),
            Span::styled(": ", text),
            Span::styled("u16", type_st),
            Span::styled(") -> ", text),
            Span::styled("bool ", type_st),
            Span::styled("{", text),
        ]),
        Line::from(vec![
            Span::styled(" 3 │ ", num_col),
            Span::styled("    let ", kw),
            Span::styled("addr = format!(", text),
            Span::styled("\"http://127.0.0.1:{}\"", literal),
            Span::styled(", port);", text),
        ]),
        Line::from(vec![
            Span::styled(" 4 │ ", num_col),
            Span::styled("    println!(", macro_st),
            Span::styled("\"Connecting to workspace engine...\"", literal),
            Span::styled(");", text),
        ]),
        Line::from(vec![
            Span::styled(" 5 │ ", num_col),
            Span::styled("    reqwest::Client::new()", text),
        ]),
        Line::from(vec![
            Span::styled(" 6 │ ", num_col),
            Span::styled("        .get(&addr)", text),
        ]),
        Line::from(vec![
            Span::styled(" 7 │ ", num_col),
            Span::styled("        .send()", text),
        ]),
        Line::from(vec![
            Span::styled(" 8 │ ", num_col),
            Span::styled("        .is_ok()", text),
        ]),
        Line::from(vec![
            Span::styled(" 9 │ ", num_col),
            Span::styled("}", text),
        ]),
    ];

    let editor_snippet = Paragraph::new(rust_code).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Live Editor Preview (Rust) ")
            .border_style(Style::default().dim())
            .bg(theme.background),
    );

    frame.render_widget(editor_snippet, chunks[9]);
}

pub fn run() -> Result<(), color_eyre::Report> {
    let themes = discover_themes();
    if themes.is_empty() {
        eprintln!("[ERR] No themes found.");
        return Ok(());
    }

    let is_tty = std::io::stdout().is_terminal();

    if !is_tty {
        println!("Available themes:");
        println!("===============================================================================");
        for t in &themes {
            let filter = if t.is_dark { "dark" } else { "light" };
            println!("  {:<30} [{}] {}", t.name, filter, t.path.display());
        }
        println!("===============================================================================");
        return Ok(());
    }

    enable_raw_mode()?;
    let result = (|| -> Result<Option<Theme>, std::io::Error> {
        let stdout = std::io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        terminal.clear()?;

        let mut filter = ThemeFilter::All;
        let mut search = String::new();
        let mut search_active = false;
        let mut state = ListState::default().with_selected(Some(0));
        let mut filtered = filter_themes(&themes, filter, &search);

        loop {
            let current_idx = state.selected().unwrap_or(0);
            let filtered_len = filtered.len();
            if current_idx >= filtered_len {
                state.select(Some(filtered_len.saturating_sub(1)));
            }

            terminal.draw(|frame| {
                let area = frame.area();
                let chunks = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Fill(1),
                    Constraint::Length(1),
                ])
                .split(area);

                let title = Line::from_iter([
                    Span::from("Select Theme").bold(),
                    Span::from(" (j/k navigate, f filter, / search, Enter confirm, Esc cancel)"),
                ]);
                frame.render_widget(Paragraph::new(title).centered(), chunks[0]);

                let filter_text = if !search.is_empty() {
                    format!(
                        "[Filter: {}] searching: \"{}\" (Press 'c' to clear)",
                        filter.label(),
                        search
                    )
                } else {
                    format!("[Filter: {}] (Press '/' to search)", filter.label())
                };
                frame.render_widget(Paragraph::new(filter_text).centered().dim(), chunks[1]);

                let main_layout =
                    Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                        .split(chunks[2]);

                let list_items: Vec<ListItem> = filtered
                    .iter()
                    .enumerate()
                    .map(|(i, &orig_idx)| {
                        let text = if i == current_idx {
                            format!("❯ {}", themes[orig_idx].name)
                        } else {
                            format!("  {}", themes[orig_idx].name)
                        };
                        ListItem::new(text)
                    })
                    .collect();

                let list = List::new(list_items)
                    .block(
                        Block::default()
                            .borders(Borders::RIGHT)
                            .border_style(Style::default().dim()),
                    )
                    .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
                    .highlight_spacing(HighlightSpacing::Always);

                frame.render_stateful_widget(list, main_layout[0], &mut state);

                if let Some(&orig_idx) = filtered.get(current_idx) {
                    render_preview(frame, &themes[orig_idx], main_layout[1]);
                }

                let footer = Line::from("Press Enter to select, q/Esc to cancel");
                frame.render_widget(Paragraph::new(footer).centered().dim(), chunks[3]);
            })?;

            if event::poll(std::time::Duration::from_millis(16))? {
                if let Event::Key(key) = event::read()? {
                    if search_active {
                        match key.code {
                            KeyCode::Esc | KeyCode::Enter => {
                                search_active = false;
                            }
                            KeyCode::Char('c') | KeyCode::Char('C') => {
                                search.clear();
                            }
                            KeyCode::Backspace => {
                                search.pop();
                            }
                            KeyCode::Char(c) => {
                                search.push(c);
                            }
                            _ => {}
                        }
                        filtered = filter_themes(&themes, filter, &search);
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('j') | KeyCode::Down => state.select_next(),
                        KeyCode::Char('k') | KeyCode::Up => state.select_previous(),
                        KeyCode::Char('g') | KeyCode::Home => state.select_first(),
                        KeyCode::Char('G') | KeyCode::End => state.select_last(),
                        KeyCode::Char('f') => {
                            filter = filter.next();
                        }
                        KeyCode::Char('/') => {
                            search_active = true;
                        }
                        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                            if let Some(idx) = state.selected() {
                                if let Some(&orig_idx) = filtered.get(idx) {
                                    return Ok(Some(themes[orig_idx].clone()));
                                }
                            }
                        }
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                        _ => {}
                    }

                    filtered = filter_themes(&themes, filter, &search);
                }
            }
        }
    })();
    disable_raw_mode()?;

    match result {
        Ok(Some(theme)) => {
            apply_theme(&theme.name)?;
            println!("[ OK ] Theme applied: {}", theme.name);
            println!("   Press Shift+Cmd+, in Ghostty to reload the configuration.");
        }
        Ok(None) => {
            println!("[INFO] Cancelled.");
        }
        Err(e) => {
            eprintln!("[ERR] {e}");
        }
    }

    Ok(())
}
