use std::{
    io::{self, Stdout},
    time::Duration,
};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph},
};
use streamer_core::{AppState, PlaybackState, ReaderCommand, SegmentStatus};
use tokio::sync::{mpsc, watch};

pub fn run_tui(
    state_rx: watch::Receiver<AppState>,
    command_tx: mpsc::UnboundedSender<ReaderCommand>,
) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let result = run_tui_loop(&mut terminal, state_rx, command_tx);
    restore_terminal(&mut terminal)?;
    result
}

fn run_tui_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state_rx: watch::Receiver<AppState>,
    command_tx: mpsc::UnboundedSender<ReaderCommand>,
) -> Result<()> {
    let state_rx = state_rx;
    let mut manual_selected: Option<usize> = None;

    loop {
        let state = state_rx.borrow().clone();
        let selected = manual_selected.unwrap_or_else(|| current_selection(&state));
        terminal.draw(|frame| render(frame, &state, selected))?;

        if matches!(
            state.playback_state,
            PlaybackState::Completed | PlaybackState::Stopped | PlaybackState::Error
        ) {
            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && matches!(key.code, KeyCode::Char('q'))
            {
                break;
            }
            break;
        }

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') => {
                    let _ = command_tx.send(ReaderCommand::Quit);
                    break;
                }
                KeyCode::Char(' ') => {
                    let _ = command_tx.send(ReaderCommand::TogglePause);
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    manual_selected = Some(
                        selected
                            .saturating_add(1)
                            .min(state.total_segments().saturating_sub(1)),
                    );
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    manual_selected = Some(selected.saturating_sub(1));
                }
                KeyCode::Char('f') => {
                    manual_selected = None;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn render(frame: &mut ratatui::Frame<'_>, state: &AppState, selected: usize) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(frame.area());

    let status = format!(
        "state={} buffered={:.1}s generated={}/{} played={}/{}",
        playback_state_label(state.playback_state),
        state.buffered_audio.as_secs_f32(),
        state.generated_segments,
        state.total_segments(),
        state.played_segments,
        state.total_segments(),
    );
    frame.render_widget(
        Paragraph::new(status).block(Block::default().borders(Borders::ALL).title("Status")),
        layout[0],
    );

    let ratio = if state.total_segments() == 0 {
        0.0
    } else {
        state.generated_segments as f64 / state.total_segments() as f64
    };
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("Generation"))
            .gauge_style(Style::default().fg(Color::Cyan))
            .ratio(ratio),
        layout[1],
    );

    let items = state
        .segments
        .iter()
        .zip(state.runtimes.iter())
        .map(|(segment, runtime)| {
            let prefix = match runtime.status {
                SegmentStatus::Pending => "  ",
                SegmentStatus::Synthesizing => "~ ",
                SegmentStatus::Buffered => "+ ",
                SegmentStatus::Playing => "> ",
                SegmentStatus::Played => "✓ ",
                SegmentStatus::Failed => "x ",
            };
            let style = style_for_status(runtime.status);
            ListItem::new(Line::from(Span::styled(
                format!("{prefix}{}", segment.text()),
                style,
            )))
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Article"))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    let mut list_state = ListState::default();
    list_state.select(Some(selected.min(state.total_segments().saturating_sub(1))));
    frame.render_stateful_widget(list, layout[2], &mut list_state);

    let footer = Paragraph::new("space pause/resume  j/k scroll  f follow  q quit")
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    frame.render_widget(footer, layout[3]);
}

fn current_selection(state: &AppState) -> usize {
    state.current_segment_id.unwrap_or_else(|| {
        state
            .played_segments
            .min(state.total_segments().saturating_sub(1))
    })
}

fn style_for_status(status: SegmentStatus) -> Style {
    match status {
        SegmentStatus::Pending => Style::default().fg(Color::DarkGray),
        SegmentStatus::Synthesizing => Style::default().fg(Color::Blue),
        SegmentStatus::Buffered => Style::default().fg(Color::Cyan),
        SegmentStatus::Playing => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        SegmentStatus::Played => Style::default().fg(Color::Green),
        SegmentStatus::Failed => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

fn playback_state_label(state: PlaybackState) -> &'static str {
    match state {
        PlaybackState::Buffering => "buffering",
        PlaybackState::Playing => "playing",
        PlaybackState::Paused => "paused",
        PlaybackState::Starved => "starved",
        PlaybackState::Completed => "completed",
        PlaybackState::Stopped => "stopped",
        PlaybackState::Error => "error",
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
