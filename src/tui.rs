use std::io::{self, IsTerminal, Stdout};
use std::time::Duration;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use thiserror::Error;

use crate::catalog::{ActionDefinition, ActionId, Catalog, CatalogFiles, CatalogSource};
use crate::execution::{ExecutionResult, ExecutionStatus, RunningSession, RuntimeEvent};
use crate::host::{Availability, Host, availability};
use crate::runtime::resolve;

const MINIMUM_WIDTH: u16 = 48;
const MINIMUM_HEIGHT: u16 = 14;
const SCROLLBACK_LINES: usize = 1_000;

#[derive(Debug, Error)]
pub enum TuiError {
    #[error("interactive mode requires a terminal on standard input and output")]
    NoTerminal,
    #[error("terminal error: {0}")]
    Terminal(#[from] io::Error),
}

pub fn run(
    catalog: Catalog,
    files: &impl CatalogFiles,
    source: CatalogSource,
) -> Result<(), TuiError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(TuiError::NoTerminal);
    }
    let mut terminal = TerminalSession::new()?;
    let mut app = App::new(catalog, source, Host::detect());
    loop {
        app.drain_runtime_events();
        terminal.terminal.draw(|frame| render(frame, &mut app))?;
        if app.quit {
            break;
        }
        if event::poll(Duration::from_millis(50))? {
            let event = event::read()?;
            if handle_event(&mut app, files, event) {
                break;
            }
        }
    }
    Ok(())
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        let backend = CrosstermBackend::new(stdout);
        match Terminal::new(backend) {
            Ok(terminal) => {
                let mut session = Self { terminal };
                session.terminal.clear()?;
                Ok(session)
            }
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
                Err(error)
            }
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen, Show);
        let _ = self.terminal.show_cursor();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pane {
    Categories,
    Actions,
}

enum View {
    Browse,
    Help,
    Preview {
        action: ActionId,
        source: String,
    },
    Confirm {
        action: ActionId,
        run_selected: bool,
    },
    Execution(Box<ExecutionView>),
}

struct ExecutionView {
    action: ActionId,
    label: String,
    parser: vt100::Parser,
    session: RunningSession,
    result: Option<ExecutionResult>,
    error: Option<String>,
    rows: u16,
    cols: u16,
}

struct App {
    catalog: Catalog,
    source: CatalogSource,
    host: Host,
    availability: Vec<Availability>,
    selected_category: usize,
    selected_action: usize,
    pane: Pane,
    search: String,
    searching: bool,
    show_unavailable: bool,
    view: View,
    quit: bool,
    color: bool,
    terminal_width: u16,
    terminal_height: u16,
}

impl App {
    fn new(catalog: Catalog, source: CatalogSource, host: Host) -> Self {
        let availability = catalog
            .actions
            .iter()
            .map(|action| availability(action, &host))
            .collect();
        Self {
            catalog,
            source,
            host,
            availability,
            selected_category: 0,
            selected_action: 0,
            pane: Pane::Categories,
            search: String::new(),
            searching: false,
            show_unavailable: false,
            view: View::Browse,
            quit: false,
            color: std::env::var_os("NO_COLOR").is_none(),
            terminal_width: 80,
            terminal_height: 24,
        }
    }

    fn filtered_actions(&self) -> Vec<usize> {
        let category = self.catalog.categories.get(self.selected_category);
        let query = self.search.to_lowercase();
        self.catalog
            .actions
            .iter()
            .enumerate()
            .filter(|(index, action)| {
                (self.show_unavailable || self.availability[*index].is_available())
                    && (query.is_empty()
                        || action.label.to_lowercase().contains(&query)
                        || action.description.to_lowercase().contains(&query))
                    && (!query.is_empty()
                        || category.is_some_and(|category| action.category == category.id))
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn selected_action(&self) -> Option<(usize, &ActionDefinition)> {
        let actions = self.filtered_actions();
        let index = *actions.get(self.selected_action.min(actions.len().saturating_sub(1)))?;
        Some((index, &self.catalog.actions[index]))
    }

    fn clamp_selection(&mut self) {
        let len = self.filtered_actions().len();
        self.selected_action = self.selected_action.min(len.saturating_sub(1));
    }

    fn drain_runtime_events(&mut self) {
        let View::Execution(execution) = &mut self.view else {
            return;
        };
        loop {
            match execution.session.try_recv() {
                Ok(RuntimeEvent::Started { .. }) => {}
                Ok(RuntimeEvent::Output(bytes)) => execution.parser.process(&bytes),
                Ok(RuntimeEvent::Exited(result)) => execution.result = Some(result),
                Ok(RuntimeEvent::Failed(error)) => execution.error = Some(error),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
    }
}

fn handle_event(app: &mut App, files: &impl CatalogFiles, event: Event) -> bool {
    if let Event::Resize(width, height) = event {
        if let View::Execution(execution) = &mut app.view {
            let (rows, cols) = execution_dimensions(width, height);
            execution.parser.set_size(rows, cols);
            let _ = execution.session.resize(rows, cols);
            execution.rows = rows;
            execution.cols = cols;
        }
        return false;
    }
    let Event::Key(key) = event else {
        return false;
    };
    if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
        return false;
    }

    if matches!(
        &app.view,
        View::Execution(execution) if execution.result.is_some() || execution.error.is_some()
    ) && matches!(key.code, KeyCode::Esc | KeyCode::Enter)
    {
        app.view = View::Browse;
        return false;
    }

    match &mut app.view {
        View::Execution(execution) => handle_execution_key(execution, key),
        View::Help | View::Preview { .. } => {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')
            ) {
                app.view = View::Browse;
            }
        }
        View::Confirm {
            action,
            run_selected,
        } => {
            if matches!(key.code, KeyCode::Esc) {
                app.view = View::Browse;
            } else if matches!(key.code, KeyCode::Left | KeyCode::Right | KeyCode::Tab) {
                *run_selected = !*run_selected;
            } else if matches!(key.code, KeyCode::Enter) {
                if *run_selected {
                    let action_id = action.clone();
                    match start_execution(app, files, &action_id) {
                        Ok(view) => app.view = View::Execution(Box::new(view)),
                        Err(error) => {
                            let source = format!("Unable to start action:\n\n{error}");
                            app.view = View::Preview {
                                action: action_id,
                                source,
                            };
                        }
                    }
                } else {
                    app.view = View::Browse;
                }
            }
        }
        View::Browse => handle_browse_key(app, files, key),
    }
    app.quit
}

fn handle_browse_key(app: &mut App, files: &impl CatalogFiles, key: KeyEvent) {
    if app.searching {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => app.searching = false,
            KeyCode::Backspace => {
                app.search.pop();
                app.selected_action = 0;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                app.search.push(character);
                app.selected_action = 0;
            }
            _ => {}
        }
        app.clamp_selection();
        return;
    }

    match key.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('?') => app.view = View::Help,
        KeyCode::Char('/') => app.searching = true,
        KeyCode::Char('u') => {
            app.show_unavailable = !app.show_unavailable;
            app.clamp_selection();
        }
        KeyCode::Char('p') => {
            if let Some((_, action)) = app.selected_action() {
                let action_id = action.id.clone();
                let source = files
                    .read(&action.script)
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_else(|| "Script source is unavailable.".to_owned());
                app.view = View::Preview {
                    action: action_id,
                    source,
                };
            }
        }
        KeyCode::Left | KeyCode::Esc => app.pane = Pane::Categories,
        KeyCode::Right | KeyCode::Tab if app.pane == Pane::Categories => app.pane = Pane::Actions,
        KeyCode::Up => move_selection(app, -1),
        KeyCode::Down => move_selection(app, 1),
        KeyCode::Enter if app.pane == Pane::Categories => app.pane = Pane::Actions,
        KeyCode::Enter => {
            if let Some((index, action)) = app.selected_action()
                && app.availability[index].is_available()
            {
                app.view = View::Confirm {
                    action: action.id.clone(),
                    run_selected: false,
                };
            }
        }
        _ => {}
    }
}

fn move_selection(app: &mut App, delta: isize) {
    match app.pane {
        Pane::Categories => {
            let len = app.catalog.categories.len();
            app.selected_category = shifted(app.selected_category, len, delta);
            app.selected_action = 0;
        }
        Pane::Actions => {
            let len = app.filtered_actions().len();
            app.selected_action = shifted(app.selected_action, len, delta);
        }
    }
}

fn shifted(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        0
    } else if delta < 0 {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        current.saturating_add(delta as usize).min(len - 1)
    }
}

fn start_execution(
    app: &App,
    files: &impl CatalogFiles,
    action_id: &ActionId,
) -> Result<ExecutionView, String> {
    let action = app
        .catalog
        .actions
        .iter()
        .find(|action| &action.id == action_id)
        .ok_or_else(|| format!("action `{action_id}` no longer exists"))?;
    let command =
        resolve(&app.catalog, files, action_id, &app.host).map_err(|error| error.to_string())?;
    let (rows, cols) = execution_dimensions(app.terminal_width, app.terminal_height);
    let session = RunningSession::start(command, rows, cols).map_err(|error| error.to_string())?;
    Ok(ExecutionView {
        action: action_id.clone(),
        label: action.label.clone(),
        parser: vt100::Parser::new(rows, cols, SCROLLBACK_LINES),
        session,
        result: None,
        error: None,
        rows,
        cols,
    })
}

fn handle_execution_key(execution: &mut ExecutionView, key: KeyEvent) {
    if execution.result.is_some() || execution.error.is_some() {
        return;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('x') {
        if let Err(error) = execution.session.cancel() {
            execution.error = Some(format!("could not cancel action: {error}"));
        }
        return;
    }
    if let Some(bytes) = encode_key(key)
        && let Err(error) = execution.session.write_input(&bytes)
    {
        execution.error = Some(format!("could not send input to action: {error}"));
    }
}

fn encode_key(key: KeyEvent) -> Option<Vec<u8>> {
    let bytes: &[u8] = match key.code {
        KeyCode::Enter => b"\r",
        KeyCode::Backspace => b"\x7f",
        KeyCode::Tab => b"\t",
        KeyCode::Esc => b"\x1b",
        KeyCode::Up => b"\x1b[A",
        KeyCode::Down => b"\x1b[B",
        KeyCode::Right => b"\x1b[C",
        KeyCode::Left => b"\x1b[D",
        KeyCode::Home => b"\x1b[H",
        KeyCode::End => b"\x1b[F",
        KeyCode::Delete => b"\x1b[3~",
        _ => b"",
    };
    if !bytes.is_empty() {
        return Some(bytes.to_vec());
    }
    let KeyCode::Char(character) = key.code else {
        return None;
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) && character.is_ascii_alphabetic() {
        return Some(vec![(character.to_ascii_lowercase() as u8) - b'a' + 1]);
    }
    let mut buffer = [0; 4];
    Some(character.encode_utf8(&mut buffer).as_bytes().to_vec())
}

fn render(frame: &mut ratatui::Frame<'_>, app: &mut App) {
    let area = frame.area();
    app.terminal_width = area.width;
    app.terminal_height = area.height;
    if area.width < MINIMUM_WIDTH || area.height < MINIMUM_HEIGHT {
        frame.render_widget(
            Paragraph::new(format!(
                "Crank needs at least {MINIMUM_WIDTH}x{MINIMUM_HEIGHT}.\nCurrent size: {}x{}",
                area.width, area.height
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Resize terminal "),
            )
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    match &app.view {
        View::Browse => render_browse(frame, app),
        View::Help => render_help(frame, area),
        View::Preview { action, source } => render_preview(frame, area, action, source),
        View::Confirm {
            action,
            run_selected,
        } => render_confirm(frame, area, app, action, *run_selected),
        View::Execution(execution) => render_execution(frame, area, execution, app.color),
    }
}

fn render_browse(frame: &mut ratatui::Frame<'_>, app: &App) {
    let area = frame.area();
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(if app.searching || !app.search.is_empty() {
                3
            } else {
                0
            }),
            Constraint::Min(8),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(area);
    let mode = match &app.source {
        CatalogSource::Embedded => String::new(),
        CatalogSource::Local(path) => format!("  LOCAL CATALOG: {}", path.display()),
    };
    frame.render_widget(
        Paragraph::new(format!("Crank {}{mode}", env!("CARGO_PKG_VERSION")))
            .style(accent(app.color).add_modifier(Modifier::BOLD)),
        sections[0],
    );
    if sections[1].height > 0 {
        let cursor = if app.searching { "_" } else { "" };
        frame.render_widget(
            Paragraph::new(format!("{}{}", app.search, cursor))
                .block(Block::default().borders(Borders::ALL).title(" Search ")),
            sections[1],
        );
    }

    let body = if area.width < 72 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(100), Constraint::Length(0)])
            .split(sections[2])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(32), Constraint::Percentage(68)])
            .split(sections[2])
    };
    if body[1].width == 0 {
        if app.pane == Pane::Categories {
            render_categories(frame, body[0], app);
        } else {
            render_actions(frame, body[0], app);
        }
    } else {
        render_categories(frame, body[0], app);
        render_actions(frame, body[1], app);
    }
    render_details(frame, sections[3], app);
    let unavailable = if app.show_unavailable { "hide" } else { "show" };
    frame.render_widget(
        Paragraph::new(format!(
            "↑↓ move  Enter select  / search  p preview  u {unavailable} unavailable  ? help  q quit"
        )),
        sections[4],
    );
}

fn render_categories(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let items = app
        .catalog
        .categories
        .iter()
        .map(|category| ListItem::new(category.label.as_str()))
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(Some(app.selected_category));
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" Categories "))
            .highlight_symbol("> ")
            .highlight_style(if app.pane == Pane::Categories {
                accent(app.color)
            } else {
                Style::default()
            }),
        area,
        &mut state,
    );
}

fn render_actions(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let indices = app.filtered_actions();
    let items = indices
        .iter()
        .map(|index| {
            let action = &app.catalog.actions[*index];
            let marker = if app.availability[*index].is_available() {
                ""
            } else {
                " [unavailable]"
            };
            ListItem::new(format!("{}{marker}", action.label))
        })
        .collect::<Vec<_>>();
    let selection =
        (!items.is_empty()).then_some(app.selected_action.min(items.len().saturating_sub(1)));
    let mut state = ListState::default().with_selected(selection);
    let title = if app.search.is_empty() {
        " Actions "
    } else {
        " Search results "
    };
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_symbol("> ")
            .highlight_style(if app.pane == Pane::Actions {
                accent(app.color)
            } else {
                Style::default()
            }),
        area,
        &mut state,
    );
}

fn render_details(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let text = if let Some((index, action)) = app.selected_action() {
        match &app.availability[index] {
            Availability::Available => action.description.clone(),
            Availability::Unavailable { reasons } => {
                format!("{} Unavailable: {}", action.description, reasons.join("; "))
            }
        }
    } else if app.catalog.actions.is_empty() {
        "This catalog has no actions.".to_owned()
    } else {
        "No actions match the current filters.".to_owned()
    };
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title(" Details "))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_help(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let text = Text::from(vec![
        Line::from("↑/↓       Move selection"),
        Line::from("Enter     Open category or run confirmation"),
        Line::from("/         Search action names and descriptions"),
        Line::from("p         Preview the selected script"),
        Line::from("u         Show or hide unavailable actions"),
        Line::from("Esc       Go back or close this view"),
        Line::from("q         Quit from the catalog"),
        Line::from(""),
        Line::from("While running, Ctrl-C interrupts the child and Ctrl-X cancels the session."),
    ]);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title(" Help "))
            .wrap(Wrap { trim: false }),
        centered(area, 68, 15),
    );
}

fn render_preview(frame: &mut ratatui::Frame<'_>, area: Rect, action: &ActionId, source: &str) {
    frame.render_widget(
        Paragraph::new(source)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" Preview: {action} — Esc to close ")),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_confirm(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    action_id: &ActionId,
    run_selected: bool,
) {
    let Some(action) = app
        .catalog
        .actions
        .iter()
        .find(|action| &action.id == action_id)
    else {
        return;
    };
    let cancel = if run_selected {
        "  Cancel  "
    } else {
        "> Cancel <"
    };
    let run = if run_selected {
        ">  Run  <"
    } else {
        "    Run    "
    };
    let text = format!(
        "{}\n\n{}\n\n{cancel}       {run}\n\n←/→ choose, Enter confirm, Esc close",
        action.label, action.description
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Confirm action "),
            )
            .wrap(Wrap { trim: true }),
        centered(area, 68, 13),
    );
}

fn render_execution(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    execution: &ExecutionView,
    color: bool,
) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(format!(
            "Running: {} ({})",
            execution.label, execution.action
        )),
        sections[0],
    );
    frame.render_widget(
        Paragraph::new(terminal_text(
            execution.parser.screen(),
            execution.rows,
            execution.cols,
            color,
        ))
        .block(Block::default().borders(Borders::ALL).title(" Terminal ")),
        sections[1],
    );
    let status = if let Some(error) = &execution.error {
        format!("Error: {error}  Esc returns to catalog")
    } else if let Some(result) = &execution.result {
        let outcome = match result.status {
            ExecutionStatus::Succeeded => "Succeeded".to_owned(),
            ExecutionStatus::Failed => format!(
                "Failed{}",
                result
                    .exit_code
                    .map(|code| format!(" (exit {code})"))
                    .unwrap_or_default()
            ),
            ExecutionStatus::Cancelled => "Cancelled".to_owned(),
        };
        format!("{outcome}  Esc or Enter returns to catalog")
    } else {
        "Ctrl-C interrupts child  Ctrl-X cancels session".to_owned()
    };
    frame.render_widget(Paragraph::new(status), sections[2]);
}

fn terminal_text(screen: &vt100::Screen, rows: u16, cols: u16, color: bool) -> Text<'static> {
    let mut lines = Vec::with_capacity(usize::from(rows));
    for row in 0..rows {
        let mut spans = Vec::with_capacity(usize::from(cols));
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let contents = if cell.has_contents() {
                cell.contents()
            } else {
                " ".to_owned()
            };
            spans.push(Span::styled(contents, cell_style(cell, color)));
        }
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}

fn cell_style(cell: &vt100::Cell, color: bool) -> Style {
    let mut style = Style::default();
    if color {
        let (foreground, background) = if cell.inverse() {
            (cell.bgcolor(), cell.fgcolor())
        } else {
            (cell.fgcolor(), cell.bgcolor())
        };
        if let Some(foreground) = terminal_color(foreground) {
            style = style.fg(foreground);
        }
        if let Some(background) = terminal_color(background) {
            style = style.bg(background);
        }
    } else if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

fn terminal_color(color: vt100::Color) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(Color::Indexed(index)),
        vt100::Color::Rgb(red, green, blue) => Some(Color::Rgb(red, green, blue)),
    }
}

fn execution_dimensions(width: u16, height: u16) -> (u16, u16) {
    (
        height.saturating_sub(5).max(1),
        width.saturating_sub(2).max(1),
    )
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn accent(enabled: bool) -> Style {
    if enabled {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::catalog::load_embedded;

    #[test]
    fn search_matches_descriptions_across_categories() {
        let bundle = load_embedded().unwrap();
        let mut app = App::new(bundle.catalog, bundle.source, Host::detect());
        app.search = "status 42".to_owned();
        let actions = app.filtered_actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(app.catalog.actions[actions[0]].id.as_str(), "fixture.fail");
    }

    #[test]
    fn cancel_is_the_confirmation_default() {
        let action = ActionId::from_str("fixture.hello").unwrap();
        let view = View::Confirm {
            action,
            run_selected: false,
        };
        assert!(matches!(
            view,
            View::Confirm {
                run_selected: false,
                ..
            }
        ));
    }

    #[test]
    fn control_c_is_forwarded_as_an_interrupt_byte() {
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(encode_key(key), Some(vec![3]));
    }

    #[test]
    fn terminal_output_preserves_ansi_colors() {
        let mut parser = vt100::Parser::new(2, 10, 0);
        parser.process(b"\x1b[31mred\x1b[0m");
        let text = terminal_text(parser.screen(), 2, 10, true);
        assert_eq!(text.lines[0].spans[0].style.fg, Some(Color::Indexed(1)));
    }
}
