use std::time::Instant;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use avix_client_core::notification::HilState;

pub fn render_hil_modal(f: &mut Frame, hil: &HilState, started_at: Instant) {
    let size = f.size();

    // Clear the entire screen for the modal
    f.render_widget(Clear, size);

    let remaining = hil
        .timeout_secs
        .saturating_sub(started_at.elapsed().as_secs() as u32);
    let mins = remaining / 60;
    let secs = remaining % 60;

    let text = format!(
        "⚠  Human Input Required\n\
         \n\
         Agent: researcher (PID 42)\n\
         Request: \"{}\"\n\
         \n\
         Timeout: {}m {}s remaining\n\
         \n\
         [A] Approve   [D] Deny   [N] Add note",
        hil.prompt, mins, secs
    );

    let para = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("HIL Request"))
        .alignment(Alignment::Center);

    let modal_size = Rect {
        x: size.width / 4,
        y: size.height / 4,
        width: size.width / 2,
        height: size.height / 2,
    };

    f.render_widget(para, modal_size);
}
