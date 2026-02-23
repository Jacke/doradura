//! Event handling: keyboard, mouse, and tick events.

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use std::time::Duration;

/// A polled input event for the main application loop.
#[derive(Debug, Clone)]
pub enum InputEvent {
    /// A keyboard event from the terminal.
    Key(KeyEvent),
    /// A mouse event (click, scroll, etc.).
    Mouse(MouseEvent),
    /// A bracketed paste event (multi-char string).
    Paste(String),
    /// The poll timeout elapsed with no input.
    Tick,
    /// The user requested a clean exit (Ctrl+C).
    Quit,
}

/// Poll for the next event, blocking for at most `tick_rate`.
pub fn next_event(tick_rate: Duration) -> std::io::Result<InputEvent> {
    if event::poll(tick_rate)? {
        match event::read()? {
            Event::Key(key) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    return Ok(InputEvent::Quit);
                }
                Ok(InputEvent::Key(key))
            }
            Event::Mouse(m) => Ok(InputEvent::Mouse(m)),
            Event::Paste(s) => Ok(InputEvent::Paste(s)),
            _ => Ok(InputEvent::Tick),
        }
    } else {
        Ok(InputEvent::Tick)
    }
}
