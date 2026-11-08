//! What the screens' unit tests share: a theme, a buffer drawn on a test
//! terminal, and where a text shows on it.

use crate::theme::Theme;
use ratatui::Frame;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

pub fn theme() -> Theme {
    Theme::builtin("dark").unwrap()
}

/// What `draw` leaves on a `width` by `height` screen.
pub fn render(width: u16, height: u16, draw: impl FnOnce(&mut Frame)) -> Buffer {
    let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(draw).unwrap();
    terminal.backend().buffer().clone()
}

/// A row's cells, each with the column it starts at: a wide character
/// takes two columns and one cell.
fn cells(buffer: &Buffer, y: u16) -> Vec<(u16, &str)> {
    let area = buffer.area;
    let mut out = vec![];
    let mut x = area.left();
    while x < area.right() {
        let symbol = buffer[(x, y)].symbol();
        out.push((x, symbol));
        x += super::text::width(symbol).max(1) as u16;
    }
    out
}

/// The text of a row, a wide character once.
pub fn row(buffer: &Buffer, y: u16) -> String {
    cells(buffer, y).into_iter().map(|(_, s)| s).collect()
}

/// The column and row where `text` starts, the first from the top.
pub fn find(buffer: &Buffer, text: &str) -> Option<(u16, u16)> {
    let area = buffer.area;
    (area.top()..area.bottom()).find_map(|y| {
        let cells = cells(buffer, y);
        (0..cells.len()).find_map(|i| {
            let rest: String = cells[i..].iter().map(|(_, s)| *s).collect();
            rest.starts_with(text).then_some((cells[i].0, y))
        })
    })
}

#[test]
fn a_wide_character_is_one_cell_two_columns_wide() {
    let buffer = render(10, 1, |f| f.render_widget(ratatui::text::Line::from("漢字 ab"), f.area()));
    assert_eq!(find(&buffer, "ab"), Some((5, 0)));
    assert_eq!(find(&buffer, "字"), Some((2, 0)));
    assert!(row(&buffer, 0).starts_with("漢字 ab"));
}
