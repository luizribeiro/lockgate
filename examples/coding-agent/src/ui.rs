//! A deliberately small terminal UI for the example agent loop.

use crate::protocol::Message;
use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::{io, time::Duration};

pub(crate) enum Action {
    Submit(String),
    Quit,
    None,
}

pub(crate) struct Ui {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    input: String,
}

impl Ui {
    pub(crate) fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, cursor::Hide)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self {
            terminal,
            input: String::new(),
        })
    }

    pub(crate) fn draw(
        &mut self,
        messages: &[Message],
        status: &str,
        workspace: &str,
    ) -> Result<()> {
        let input = &self.input;
        self.terminal
            .draw(|frame| draw(frame, messages, input, status, workspace))?;
        Ok(())
    }

    pub(crate) fn next_action(&mut self) -> Result<Action> {
        if !event::poll(Duration::from_millis(100))? {
            return Ok(Action::None);
        }
        let Event::Key(key) = event::read()? else {
            return Ok(Action::None);
        };
        if key.kind != KeyEventKind::Press {
            return Ok(Action::None);
        }
        if key.code == KeyCode::Esc
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return Ok(Action::Quit);
        }
        match key.code {
            KeyCode::Enter if !self.input.trim().is_empty() => {
                Ok(Action::Submit(std::mem::take(&mut self.input)))
            }
            KeyCode::Backspace => {
                self.input.pop();
                Ok(Action::None)
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.input.push(character);
                Ok(Action::None)
            }
            _ => Ok(Action::None),
        }
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            cursor::Show
        );
    }
}

fn draw(frame: &mut Frame<'_>, messages: &[Message], input: &str, status: &str, workspace: &str) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(format!("Workspace: {workspace}")).block(
            Block::default()
                .title(" Lockgate coding agent ")
                .borders(Borders::ALL),
        ),
        areas[0],
    );

    let lines = transcript(messages);
    let available = areas[1].height.saturating_sub(2) as usize;
    let width = areas[1].width.saturating_sub(2).max(1) as usize;
    let rendered_lines = lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum::<usize>();
    let conversation = Paragraph::new(lines).wrap(Wrap { trim: false });
    let scroll = rendered_lines
        .saturating_sub(available)
        .min(u16::MAX as usize) as u16;
    frame.render_widget(
        conversation
            .block(
                Block::default()
                    .title(" Conversation ")
                    .borders(Borders::ALL),
            )
            .scroll((scroll, 0)),
        areas[1],
    );
    frame.render_widget(
        Paragraph::new(input)
            .style(Style::default().fg(Color::White))
            .block(Block::default().title(" Prompt ").borders(Borders::ALL)),
        areas[2],
    );
    frame.render_widget(
        Paragraph::new(format!("{status}  •  Enter send  •  Esc/Ctrl-C quit"))
            .style(Style::default().fg(Color::DarkGray)),
        areas[3],
    );
    frame.set_cursor_position((
        areas[2].x + input.chars().count() as u16 + 1,
        areas[2].y + 1,
    ));
}

fn transcript(messages: &[Message]) -> Vec<Line<'static>> {
    messages
        .iter()
        .filter(|message| message.role != "system")
        .flat_map(|message| {
            let (label, color) = match message.role.as_str() {
                "user" => ("you", Color::Cyan),
                "assistant" => ("agent", Color::Green),
                "tool" => (message.name.as_deref().unwrap_or("tool"), Color::Yellow),
                role => (role, Color::Magenta),
            };
            let body = if message.content.is_empty() && !message.tool_calls.is_empty() {
                format!(
                    "calling {}",
                    message
                        .tool_calls
                        .iter()
                        .map(|call| call.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                message.content.clone()
            };
            let mut lines = vec![Line::from(Span::styled(
                format!("{label}:"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ))];
            lines.extend(body.lines().map(|line| Line::from(line.to_owned())));
            lines.push(Line::default());
            lines
        })
        .collect()
}
