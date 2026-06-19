use crossterm::event::{self, KeyCode};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{HighlightSpacing, List, ListItem, ListState, Row, Table, TableState};
use ratatui::Frame;
use std::panic::{self, AssertUnwindSafe};

pub struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

pub fn select_list(items: &[&str]) -> Option<usize> {
    if items.is_empty() {
        return None;
    }

    let _guard = TerminalGuard;
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        ratatui::run::<_, Result<_, color_eyre::Report>>(|terminal| {
            let mut state = ListState::default().with_selected(Some(0));
            loop {
                terminal.draw(|frame| render_list(frame, items, &mut state, "Select"))?;

                if let Some(key) = event::read()?.as_key_press_event() {
                    match key.code {
                        KeyCode::Char('j') | KeyCode::Down => state.select_next(),
                        KeyCode::Char('k') | KeyCode::Up => state.select_previous(),
                        KeyCode::Char('g') | KeyCode::Home => state.select_first(),
                        KeyCode::Char('G') | KeyCode::End => state.select_last(),
                        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                            if let Some(idx) = state.selected() {
                                return Ok(Some(idx));
                            }
                        }
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                        _ => {}
                    }
                }
            }
        })
    }));

    match result {
        Ok(Ok(opt)) => opt,
        _ => None,
    }
}

fn render_list(frame: &mut Frame, items: &[&str], state: &mut ListState, label: &str) {
    let constraints = [
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ];
    let layout = Layout::vertical(constraints).spacing(1);
    let areas: [Rect; 3] = layout.areas(frame.area());
    let [header_area, list_area, footer_area] = areas;

    let title = Line::from_iter([
        Span::from(format!("Select {}", label)).bold(),
        Span::from(" (j/k navigate, Enter confirm, Esc cancel)"),
    ]);
    frame.render_widget(title.centered(), header_area);

    let list_items: Vec<ListItem> = items.iter().map(|item| ListItem::new(*item)).collect();
    let list = List::new(list_items)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ")
        .highlight_spacing(HighlightSpacing::Always);

    frame.render_stateful_widget(list, list_area, state);

    let footer = Line::from("Press Enter to select, q/Esc to cancel");
    frame.render_widget(footer.centered(), footer_area);
}

pub fn select_table(headers: &[&str], rows: Vec<Vec<String>>) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }

    let _guard = TerminalGuard;
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        ratatui::run::<_, Result<_, color_eyre::Report>>(|terminal| {
            let mut state = TableState::default().with_selected(Some(0));
            loop {
                terminal.draw(|frame| render_table(frame, headers, &rows, &mut state))?;

                if let Some(key) = event::read()?.as_key_press_event() {
                    match key.code {
                        KeyCode::Char('j') | KeyCode::Down => state.select_next(),
                        KeyCode::Char('k') | KeyCode::Up => state.select_previous(),
                        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                            if let Some(idx) = state.selected() {
                                return Ok(Some(idx));
                            }
                        }
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                        _ => {}
                    }
                }
            }
        })
    }));

    match result {
        Ok(Ok(opt)) => opt,
        _ => None,
    }
}

fn render_table(frame: &mut Frame, headers: &[&str], rows: &[Vec<String>], state: &mut TableState) {
    let constraints = [
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ];
    let layout = Layout::vertical(constraints).spacing(1);
    let areas: [Rect; 3] = layout.areas(frame.area());
    let [header_area, table_area, footer_area] = areas;

    let title = Line::from_iter([
        Span::from("Select Entry").bold(),
        Span::from(" (j/k navigate, Enter confirm, Esc cancel)"),
    ]);
    frame.render_widget(title.centered(), header_area);

    let widths: Vec<Constraint> = headers.iter().map(|_| Constraint::Length(20)).collect();
    let table_rows: Vec<Row> = rows
        .iter()
        .map(|row| Row::new(row.iter().map(|s| Line::from(s.as_str()))))
        .collect();

    let table = Table::new(table_rows, widths)
        .header(Row::new(headers.iter().map(|h| Line::from(*h))))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ")
        .highlight_spacing(HighlightSpacing::Always);

    frame.render_stateful_widget(table, table_area, state);

    let footer = Line::from("Press Enter to select, q/Esc to cancel");
    frame.render_widget(footer.centered(), footer_area);
}
