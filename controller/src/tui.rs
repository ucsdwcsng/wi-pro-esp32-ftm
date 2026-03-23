use std::io;
use tokio::sync::mpsc;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::Style,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Terminal,
};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use log::{Log, Metadata, Record};

use crate::srv::*;

#[derive(Debug, Clone)]
pub enum Command {
    ListClients,
    SendBroadcast(String),
    SendToClient(String, String),
    Shutdown,
}


struct App {
    logs: Vec<String>,
    input: String,
    clients: Vec<String>,
    log_scroll: usize
}

impl App {
    fn new() -> App {
        App {
            logs: Vec::new(),
            input: String::new(),
            clients: Vec::new(),
	    log_scroll: 0
        }
    }

    fn add_log(&mut self, log: String) {
        self.logs.push(log);
        if self.logs.len() > 100 {
            self.logs.remove(0);
        }
	self.log_scroll = self.logs.len().saturating_sub(1);
    }
}

pub async fn run_tui(
    cmd_tx: mpsc::Sender<Command>,
    mut event_rx: mpsc::Receiver<ServerEvent>,
) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    app.add_log("TUI started. Commands: 'list', 'broadcast <msg>', 'quit'".to_string());

    loop {
        // Check for server events
        while let Ok(event) = event_rx.try_recv() {
            match event {
                ServerEvent::ClientConnected(name) => {
                    app.add_log(format!("✓ Client connected: {}", name));
                }
                ServerEvent::ClientMessage(name, msg) => {
                    app.add_log(format!("[{}] {}", name, msg));
                }
                ServerEvent::ClientList(clients) => {
                    app.clients = clients.clone();
                    app.add_log(format!("Clients ({}): {}", clients.len(), clients.join(", ")));
                }
                ServerEvent::Log(log) => {
                    app.add_log(log);
                }
            }
        }

        // Draw UI
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(3),
                ].as_ref())
                .split(f.area());

            // Log area
            let logs: Vec<ListItem> = app.logs.iter()
                .map(|log| ListItem::new(log.as_str()))
                .collect();
            let logs_widget = List::new(logs)
                .block(Block::default()
                    .title("Server Logs")
                    .borders(Borders::ALL));
               let mut list_state = ListState::default();
	    list_state.select(Some(app.log_scroll));
	    f.render_stateful_widget(logs_widget, chunks[0], &mut list_state);

            // Input area
            let input_widget = Paragraph::new(app.input.as_str())
                .style(Style::default())
                .block(Block::default()
                    .title("Command")
                    .borders(Borders::ALL));
            f.render_widget(input_widget, chunks[1]);
        })?;

        // Handle input
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char(c) => {
                        app.input.push(c);
                    }
                    KeyCode::Backspace => {
                        app.input.pop();
                    }
                    KeyCode::Enter => {
                        let input = app.input.clone();
                        app.input.clear();

                        let parts: Vec<&str> = input.split_whitespace().collect();
                        if parts.is_empty() {
                            continue;
                        }

                        match parts[0] {
                            "list" => {
                                cmd_tx.send(Command::ListClients).await.ok();
                            }
                            "broadcast" if parts.len() > 1 => {
                                let msg = parts[1..].join(" ");
                                cmd_tx.send(Command::SendBroadcast(msg)).await.ok();
                            }
                            "quit" | "exit" => {
                                cmd_tx.send(Command::Shutdown).await.ok();
                                break;
                            }
                            _ => {
                                app.add_log(format!("Unknown command: {}", input));
                            }
                        }
                    }
                    KeyCode::Esc => {
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    // Cleanup
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

pub struct TuiLogger {
    sender: mpsc::Sender<ServerEvent>,
}

impl Log for TuiLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let msg = format!("[{}] {}", record.level(), record.args());
        // try_send is sync and non-blocking — safe to use here.
        // If the channel is full, the log message is just dropped.
        let _ = self.sender.try_send(ServerEvent::Log(msg));
    }

    fn flush(&self) {}
}

pub fn init_tui_logger(sender: mpsc::Sender<ServerEvent>, level: log::LevelFilter) {
    let logger = TuiLogger { sender };
    log::set_boxed_logger(Box::new(logger)).expect("failed to set TUI logger");
    log::set_max_level(level);
}
