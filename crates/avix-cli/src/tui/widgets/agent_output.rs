use std::collections::VecDeque;
use std::time::Instant;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

#[derive(Debug, Clone)]
pub struct AgentOutputBuffer {
    /// Circular buffer — keeps the last MAX_LINES lines
    lines: VecDeque<String>,
    pub scroll_offset: u16,
    /// Set when a message was sent and we're waiting for the first response chunk.
    processing_since: Option<Instant>,
}

impl Default for AgentOutputBuffer {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
            scroll_offset: 0,
            processing_since: None,
        }
    }
}

const MAX_LINES: usize = 5000;

impl AgentOutputBuffer {
    pub fn push_text(&mut self, text: &str) {
        for line in text.split_inclusive('\n') {
            let line = line.trim_end_matches('\n').to_string();
            self.lines.push_back(line);
            if self.lines.len() > MAX_LINES {
                self.lines.pop_front();
            }
        }
    }

    pub fn push_user_message(&mut self, text: &str) {
        self.push_text(&format!("> {}\n", text));
    }

    pub fn set_processing(&mut self, processing: bool) {
        self.processing_since = if processing { Some(Instant::now()) } else { None };
    }

    #[allow(dead_code)]
    pub fn visible_lines(&self, height: u16) -> Vec<&str> {
        let start = self.scroll_offset as usize;
        self.lines
            .iter()
            .skip(start)
            .take(height as usize)
            .map(|s| s.as_str())
            .collect()
    }

    #[allow(dead_code)]
    pub fn scroll_down(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
        let max_offset = self.lines.len().saturating_sub(1);
        if self.scroll_offset as usize > max_offset {
            self.scroll_offset = max_offset as u16;
        }
    }

    #[allow(dead_code)]
    pub fn scroll_up(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    #[allow(dead_code)]
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.lines.len().saturating_sub(1) as u16;
    }

    // For testing
    #[cfg(test)]
    pub fn lines_len(&self) -> usize {
        self.lines.len()
    }

    pub fn render(&self, pid: u64, area: Rect) -> Paragraph<'_> {
        let visible = self.visible_lines(area.height);
        let mut text = visible.join("\n");
        if let Some(since) = self.processing_since {
            let frame = (since.elapsed().as_millis() / 250 % 4) as usize;
            let spinner = ["|", "/", "-", "\\"][frame];
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("{} Processing...", spinner));
        }
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!("Agent {} Output", pid)),
            )
            .wrap(Wrap { trim: false })
    }

    #[cfg(test)]
    pub fn get_line(&self, i: usize) -> Option<&str> {
        self.lines.get(i).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_text_splits_on_newlines() {
        let mut buf = AgentOutputBuffer::default();
        buf.push_text("line1\nline2\nline3");
        assert_eq!(buf.lines.len(), 3);
    }

    #[test]
    fn push_text_respects_max_lines() {
        let mut buf = AgentOutputBuffer::default();
        for i in 0..MAX_LINES + 10 {
            buf.push_text(&format!("line {}\n", i));
        }
        assert_eq!(buf.lines.len(), MAX_LINES);
    }

    #[test]
    fn scroll_to_bottom_shows_last_line() {
        let mut buf = AgentOutputBuffer::default();
        for i in 0..100 {
            buf.push_text(&format!("line {}\n", i));
        }
        buf.scroll_to_bottom();
        let visible = buf.visible_lines(10);
        assert_eq!(visible.last().unwrap(), &"line 99");
    }

    #[test]
    fn scroll_up_saturates_at_zero() {
        let mut buf = AgentOutputBuffer::default();
        buf.push_text("only one line\n");
        buf.scroll_up(100);
        assert_eq!(buf.scroll_offset, 0);
    }

    #[test]
    fn render_shows_visible_lines() {
        let mut buf = AgentOutputBuffer::default();
        buf.push_text("line1\nline2\nline3\n");
        let area = Rect::new(0, 0, 50, 2);
        let _para = buf.render(123, area);
        // Can't check content, but ensure it doesn't panic
    }
}
