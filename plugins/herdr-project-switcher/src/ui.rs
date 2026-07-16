use std::collections::HashMap;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use ansi_to_tui::IntoText;
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Terminal;

use crate::git_status::{FileState, FileStateIndex, ProjectGitStatus, RepoState};
use crate::projects::Project;

type Tui = Terminal<CrosstermBackend<Stdout>>;

pub fn pick(projects: &[Project]) -> Result<Option<PathBuf>> {
    let mut terminal = open_terminal()?;
    let _guard = TerminalGuard;
    let mut query = String::new();
    let mut selected = 0usize;
    let mut preview_cache: Option<(PathBuf, Vec<Line<'static>>)> = None;
    let status_updates =
        crate::git_status::scan(projects.iter().map(|project| project.path.clone()));
    let mut statuses = HashMap::new();

    loop {
        while let Ok((path, status)) = status_updates.try_recv() {
            statuses.insert(path, status);
        }
        let matches = filtered(projects, &query);
        if selected >= matches.len() {
            selected = matches.len().saturating_sub(1);
        }
        let selected_path = matches
            .get(selected)
            .map(|index| projects[*index].path.as_path());
        if preview_cache.as_ref().map(|(path, _)| path.as_path()) != selected_path {
            preview_cache = selected_path.map(|path| (path.to_path_buf(), preview(path)));
        }
        terminal.draw(|frame| {
            let area = frame.area();
            let vertical = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(1),
                    Constraint::Length(1),
                ])
                .split(area);
            frame.render_widget(
                Paragraph::new(format!(" {query}"))
                    .block(Block::default().borders(Borders::ALL).title("Project")),
                vertical[0],
            );
            frame.set_cursor_position((
                (vertical[0].x + query.chars().count() as u16 + 2)
                    .min(vertical[0].right().saturating_sub(2)),
                vertical[0].y + 1,
            ));

            let columns = if vertical[1].width >= 80 {
                Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
                    .split(vertical[1])
            } else {
                Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(100), Constraint::Percentage(0)])
                    .split(vertical[1])
            };
            let items: Vec<ListItem> = matches
                .iter()
                .map(|index| {
                    let project = &projects[*index];
                    project_item(project, statuses.get(&project.path))
                })
                .collect();
            let mut state =
                ListState::default().with_selected((!matches.is_empty()).then_some(selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(format!("Projects ({})", matches.len())),
                    )
                    .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                    .highlight_symbol("› "),
                columns[0],
                &mut state,
            );
            if columns[1].width > 1 {
                let preview = preview_cache
                    .as_ref()
                    .map(|(_, lines)| lines.clone())
                    .unwrap_or_default();
                frame.render_widget(
                    Paragraph::new(preview)
                        .block(Block::default().borders(Borders::ALL).title("Preview"))
                        .wrap(Wrap { trim: false }),
                    columns[1],
                );
            }
            frame.render_widget(
                Paragraph::new("enter: switch  ↑↓: select  esc: close"),
                vertical[2],
            );
        })?;

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
            KeyCode::Enter => {
                return Ok(matches
                    .get(selected)
                    .map(|index| projects[*index].path.clone()));
            }
            KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Down => {
                if selected + 1 < matches.len() {
                    selected += 1;
                }
            }
            KeyCode::Backspace => {
                query.pop();
                selected = 0;
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                query.push(character);
                selected = 0;
            }
            _ => {}
        }
    }
}

/// Ask for a project root when no configured or conventional root is usable.
pub fn choose_root() -> Result<Option<PathBuf>> {
    let mut terminal = open_terminal()?;
    let _guard = TerminalGuard;
    let mut input = dirs::home_dir()
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut error = String::new();

    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(3),
                    Constraint::Length(2),
                    Constraint::Min(1),
                ])
                .split(area);
            frame.render_widget(
                Paragraph::new("No project directories were found. Enter a directory whose immediate children are projects."),
                rows[0],
            );
            frame.render_widget(
                Paragraph::new(input.as_str())
                    .block(Block::default().borders(Borders::ALL).title("Project root")),
                rows[1],
            );
            frame.set_cursor_position((
                (rows[1].x + input.chars().count() as u16 + 1)
                    .min(rows[1].right().saturating_sub(2)),
                rows[1].y + 1,
            ));
            frame.render_widget(
                Paragraph::new(if error.is_empty() {
                    "enter: save  esc: close"
                } else {
                    error.as_str()
                }),
                rows[2],
            );
        })?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
            KeyCode::Enter => {
                let path = crate::projects::expand_tilde(input.trim());
                if path.is_dir() {
                    return Ok(Some(path));
                }
                error = format!("Not a directory: {}", path.display());
            }
            KeyCode::Backspace => {
                input.pop();
                error.clear();
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                input.push(character);
                error.clear();
            }
            _ => {}
        }
    }
}

fn open_terminal() -> Result<Tui> {
    enable_raw_mode().context("enabling terminal raw mode")?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(error).context("entering alternate screen");
    }
    match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => Ok(terminal),
        Err(error) => {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            Err(error).context("opening terminal UI")
        }
    }
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn project_item(project: &Project, status: Option<&ProjectGitStatus>) -> ListItem<'static> {
    let (name_style, mut status_spans) = match status {
        None => (
            Style::default(),
            vec![Span::styled(
                "  …",
                Style::default().add_modifier(Modifier::DIM),
            )],
        ),
        Some(ProjectGitStatus {
            state: RepoState::Clean,
            ..
        }) => (
            Style::default().fg(Color::Green),
            vec![Span::styled("  clean", Style::default().fg(Color::Green))],
        ),
        Some(
            status @ ProjectGitStatus {
                state: RepoState::Changed,
                ..
            },
        ) => (
            Style::default().fg(Color::Yellow),
            change_spans(status, false),
        ),
        Some(
            status @ ProjectGitStatus {
                state: RepoState::Conflicted,
                ..
            },
        ) => (
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            change_spans(status, true),
        ),
        Some(ProjectGitStatus {
            state: RepoState::NotRepository,
            ..
        }) => (Style::default(), Vec::new()),
    };
    let mut spans = vec![
        Span::styled(project.name.clone(), name_style),
        Span::styled(
            format!(
                "  {}",
                project.path.parent().unwrap_or(Path::new("")).display()
            ),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ];
    spans.append(&mut status_spans);
    ListItem::new(Line::from(spans))
}

fn change_spans(status: &ProjectGitStatus, conflicted: bool) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if conflicted {
        spans.push(Span::styled(
            "  conflict",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    spans.extend([
        Span::styled(
            format!("  +{}", status.added),
            Style::default().fg(Color::Green),
        ),
        Span::styled(
            format!(" -{}", status.removed),
            Style::default().fg(Color::Red),
        ),
    ]);
    if status.untracked > 0 {
        spans.push(Span::styled(
            format!(" ?{}", status.untracked),
            Style::default().fg(Color::Cyan),
        ));
    }
    spans
}

fn filtered(projects: &[Project], query: &str) -> Vec<usize> {
    let mut matches: Vec<_> = projects
        .iter()
        .enumerate()
        .filter_map(|(index, project)| {
            fuzzy_score(&project.name, query).map(|score| (index, score))
        })
        .collect();
    matches.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_index.cmp(right_index))
    });
    matches.into_iter().map(|(index, _)| index).collect()
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let candidate = candidate.to_lowercase();
    let query = query.to_lowercase();
    let mut position = 0usize;
    let mut previous = None;
    let mut score = 0i64;
    for wanted in query.chars() {
        let offset = candidate[position..].find(wanted)?;
        let found = position + offset;
        score -= found as i64;
        if previous == Some(found.saturating_sub(1)) {
            score += 15;
        }
        previous = Some(found);
        position = found + wanted.len_utf8();
    }
    Some(score)
}

fn preview(path: &Path) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let file_states = crate::git_status::file_states(path);
    if let Some(mut git) = git_status(path) {
        lines.push(Line::styled(
            "Git status",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        git.truncate(12);
        lines.extend(git);
        lines.push(Line::default());
    }
    lines.push(Line::styled(
        "Files",
        Style::default().add_modifier(Modifier::BOLD),
    ));
    lines.extend(tree_lines(path, &file_states));
    let readme = path.join("README.md");
    if let Ok(text) = std::fs::read_to_string(readme) {
        lines.push(Line::default());
        lines.push(Line::styled(
            "README",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        lines.extend(
            text.lines()
                .take(15)
                .map(|line| Line::from(line.to_owned())),
        );
    }
    lines
}

fn tree_lines(root: &Path, states: &FileStateIndex) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    let mut entries = sorted_entries(root);
    entries.truncate(12);
    for path in entries {
        let is_dir = path.is_dir();
        result.push(tree_line(root, &path, "", states));
        if is_dir && result.len() < 20 {
            let mut children = sorted_entries(&path);
            children.truncate(4);
            for child in children {
                result.push(tree_line(root, &child, "  ", states));
                if result.len() >= 20 {
                    break;
                }
            }
        }
    }
    result
}

fn tree_line(root: &Path, path: &Path, indent: &str, states: &FileStateIndex) -> Line<'static> {
    let directory = path.is_dir();
    let relative = path.strip_prefix(root).unwrap_or(path);
    let style = states
        .state_for(relative, directory)
        .map(file_state_style)
        .unwrap_or_default();
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    Line::from(Span::styled(
        format!("{indent}{} {name}", if directory { "▸" } else { " " }),
        style,
    ))
}

fn file_state_style(state: FileState) -> Style {
    match state {
        FileState::Staged => Style::default().fg(Color::Green),
        FileState::Modified => Style::default().fg(Color::Yellow),
        FileState::Untracked => Style::default().fg(Color::Cyan),
        FileState::Conflicted => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

fn sorted_entries(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
        })
        .collect();
    paths.sort_by(|left, right| {
        right
            .is_dir()
            .cmp(&left.is_dir())
            .then_with(|| left.file_name().cmp(&right.file_name()))
    });
    paths
}

fn git_status(path: &Path) -> Option<Vec<Line<'static>>> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["-c", "color.status=always", "status", "--short", "--branch"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    output.stdout.into_text().ok().map(|text| text.lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_match_is_subsequence_based() {
        assert!(fuzzy_score("project-switcher", "psw").is_some());
        assert!(fuzzy_score("project-switcher", "xyz").is_none());
    }

    #[test]
    fn contiguous_matches_rank_higher() {
        assert!(fuzzy_score("switcher", "swi") > fuzzy_score("some-wide-index", "swi"));
    }

    #[test]
    fn ansi_git_status_keeps_its_colors() {
        let text = b"## \x1b[32mmain\x1b[m\n \x1b[31mM\x1b[m file.rs\n"
            .into_text()
            .unwrap();
        assert!(text.lines.iter().flat_map(|line| &line.spans).any(|span| {
            matches!(
                span.style.fg,
                Some(ratatui::style::Color::Green | ratatui::style::Color::Red)
            )
        }));
    }
}
