use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, GridCell};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use alacritty_terminal::vte::ansi::{Color, CursorShape as AlacrittyCursorShape, NamedColor};
use alacritty_terminal::Term;

use crate::remote::model::TerminalSize;
use crate::terminal::model::{
    CellAttributes, CursorShape, CursorState, Revision, RowPatch, ScreenCell, ScreenDiff,
    ScreenRow, ScreenSnapshot, TerminalColor, TerminalModes,
};
use crate::terminal::MAX_SCROLLBACK_ROWS;

const MAX_COMBINING_SCALARS_PER_CELL: usize = 32;

pub struct ScreenDamage {
    pub diff: Option<ScreenDiff>,
    pub replies: Vec<Vec<u8>>,
    pub title: Option<String>,
    pub bell: bool,
}

#[derive(Clone, Default)]
struct ScreenEvents(Arc<Mutex<Vec<Event>>>);

impl ScreenEvents {
    fn drain(&self) -> Vec<Event> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
            .collect()
    }
}

impl EventListener for ScreenEvents {
    fn send_event(&self, event: Event) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(event);
    }
}

#[derive(Clone, Copy)]
struct ScreenDimensions {
    cols: usize,
    rows: usize,
}

impl From<TerminalSize> for ScreenDimensions {
    fn from(size: TerminalSize) -> Self {
        Self {
            cols: usize::from(size.cols()),
            rows: usize::from(size.rows()),
        }
    }
}

impl Dimensions for ScreenDimensions {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct ScreenModel {
    processor: Processor,
    term: Term<ScreenEvents>,
    events: ScreenEvents,
    size: TerminalSize,
    revision: Revision,
    last_snapshot_tab: RefCell<Option<String>>,
}

impl ScreenModel {
    pub fn new(size: TerminalSize) -> Self {
        let events = ScreenEvents::default();
        let config = Config {
            scrolling_history: MAX_SCROLLBACK_ROWS,
            ..Config::default()
        };
        let mut term = Term::new(config, &ScreenDimensions::from(size), events.clone());
        term.reset_damage();

        Self {
            processor: Processor::new(),
            term,
            events,
            size,
            revision: Revision::default(),
            last_snapshot_tab: RefCell::new(None),
        }
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    pub fn process(&mut self, bytes: &[u8]) -> ScreenDamage {
        self.processor.advance(&mut self.term, bytes);
        self.collect_damage(false)
    }

    pub fn resize(&mut self, size: TerminalSize) -> ScreenDamage {
        if size != self.size {
            self.term.resize(ScreenDimensions::from(size));
            self.size = size;
            self.revision.0 = self.revision.0.saturating_add(1);
        }

        let mut damage = self.collect_damage(true);
        // ScreenDiff intentionally has no dimensions: a resize invalidates the
        // client's shape and therefore requires a fresh snapshot.
        damage.diff = None;
        damage
    }

    pub fn snapshot(&self, tab: &str) -> ScreenSnapshot {
        self.last_snapshot_tab.replace(Some(tab.to_owned()));
        ScreenSnapshot::new(
            tab,
            self.revision,
            self.size,
            self.visible_rows(),
            Vec::new(),
            cursor_state(&self.term),
            terminal_modes(&self.term),
        )
    }

    pub fn scrollback_page(&self, offset: usize, count: usize) -> Vec<ScreenRow> {
        let history_size = self.term.grid().history_size().min(MAX_SCROLLBACK_ROWS);
        let start = offset.min(history_size);
        let count = count.min(MAX_SCROLLBACK_ROWS).min(history_size - start);

        (start..start + count)
            .map(|index| {
                let line = Line(-i32::try_from(index + 1).expect("scrollback is bounded"));
                row_from_grid(&self.term, line)
            })
            .collect()
    }

    fn collect_damage(&mut self, resized: bool) -> ScreenDamage {
        let damaged_rows: Vec<usize> = match self.term.damage() {
            TermDamage::Full => (0..usize::from(self.size.rows())).collect(),
            TermDamage::Partial(lines) => lines
                .map(|damage| damage.line)
                .filter(|line| *line < usize::from(self.size.rows()))
                .collect(),
        };
        self.term.reset_damage();

        let mut replies = Vec::new();
        let mut title = None;
        let mut bell = false;
        for event in self.events.drain() {
            match event {
                Event::PtyWrite(reply) => replies.push(reply.into_bytes()),
                Event::Title(new_title) => title = Some(new_title),
                Event::ResetTitle => title = Some(String::new()),
                Event::Bell => bell = true,
                // Remote terminal state never reads or writes the host clipboard.
                Event::ClipboardStore(..) | Event::ClipboardLoad(..) => {}
                _ => {}
            }
        }

        let diff = if resized || damaged_rows.is_empty() {
            None
        } else {
            let base_revision = self.revision;
            self.revision.0 = self.revision.0.saturating_add(1);
            self.last_snapshot_tab.borrow().as_ref().map(|tab| {
                let rows = damaged_rows
                    .into_iter()
                    .map(|row| {
                        RowPatch::new(
                            u16::try_from(row).expect("terminal rows are bounded to 512"),
                            row_from_grid(&self.term, Line(row as i32)),
                        )
                    })
                    .collect();
                ScreenDiff::for_tab(tab, base_revision, self.revision, rows)
                    .with_cursor(cursor_state(&self.term))
                    .with_modes(terminal_modes(&self.term))
            })
        };

        ScreenDamage {
            diff,
            replies,
            title,
            bell,
        }
    }

    fn visible_rows(&self) -> Vec<ScreenRow> {
        (0..self.size.rows())
            .map(|row| row_from_grid(&self.term, Line(i32::from(row))))
            .collect()
    }
}

fn row_from_grid(term: &Term<ScreenEvents>, line: Line) -> ScreenRow {
    let grid_row = &term.grid()[line];
    let wrapped = grid_row[Column(grid_row.len() - 1)]
        .flags
        .contains(Flags::WRAPLINE);
    let last_occupied = grid_row
        .into_iter()
        .rposition(|cell| !cell.is_empty())
        .map_or(0, |index| index + 1);
    let mut cells = Vec::with_capacity(last_occupied);
    let mut column = 0;

    while column < last_occupied {
        let cell = &grid_row[Column(column)];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            // Alacritty maintains paired wide cells, but sanitizing here keeps
            // malformed escape/reflow input from crossing AITerm's row contract.
            cells.push(regular_cell(cell, 1));
            column += 1;
            continue;
        }

        let is_wide = cell.flags.contains(Flags::WIDE_CHAR)
            && column + 1 < grid_row.len()
            && grid_row[Column(column + 1)]
                .flags
                .contains(Flags::WIDE_CHAR_SPACER);
        cells.push(regular_cell(cell, if is_wide { 2 } else { 1 }));
        if is_wide {
            cells.push(ScreenCell::continuation());
            column += 2;
        } else {
            column += 1;
        }
    }

    ScreenRow::try_new(cells, wrapped).expect("adapter constructs valid wide-cell pairs")
}

fn regular_cell(cell: &Cell, width: u8) -> ScreenCell {
    let mut text = cell.c.to_string();
    if let Some(zerowidth) = cell.zerowidth() {
        for scalar in zerowidth.iter().take(MAX_COMBINING_SCALARS_PER_CELL) {
            text.push(*scalar);
        }
    }

    ScreenCell::try_new(
        text,
        width,
        terminal_color(cell.fg),
        terminal_color(cell.bg),
        CellAttributes::new(
            cell.flags.contains(Flags::BOLD),
            cell.flags.contains(Flags::DIM),
            cell.flags.contains(Flags::ITALIC),
            cell.flags.intersects(Flags::ALL_UNDERLINES),
            cell.flags.contains(Flags::INVERSE),
            cell.flags.contains(Flags::HIDDEN),
            cell.flags.contains(Flags::STRIKEOUT),
        ),
    )
    .expect("adapter emits only single- and double-width cells")
}

fn terminal_color(color: Color) -> TerminalColor {
    match color {
        Color::Spec(rgb) => TerminalColor::Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        },
        Color::Indexed(index) => TerminalColor::Indexed(index),
        Color::Named(named) => named_color(named),
    }
}

fn named_color(color: NamedColor) -> TerminalColor {
    match color {
        NamedColor::Black
        | NamedColor::Red
        | NamedColor::Green
        | NamedColor::Yellow
        | NamedColor::Blue
        | NamedColor::Magenta
        | NamedColor::Cyan
        | NamedColor::White
        | NamedColor::BrightBlack
        | NamedColor::BrightRed
        | NamedColor::BrightGreen
        | NamedColor::BrightYellow
        | NamedColor::BrightBlue
        | NamedColor::BrightMagenta
        | NamedColor::BrightCyan
        | NamedColor::BrightWhite => TerminalColor::Indexed(color as u8),
        NamedColor::DimBlack => TerminalColor::Indexed(0),
        NamedColor::DimRed => TerminalColor::Indexed(1),
        NamedColor::DimGreen => TerminalColor::Indexed(2),
        NamedColor::DimYellow => TerminalColor::Indexed(3),
        NamedColor::DimBlue => TerminalColor::Indexed(4),
        NamedColor::DimMagenta => TerminalColor::Indexed(5),
        NamedColor::DimCyan => TerminalColor::Indexed(6),
        NamedColor::DimWhite => TerminalColor::Indexed(7),
        _ => TerminalColor::Default,
    }
}

fn cursor_state(term: &Term<ScreenEvents>) -> CursorState {
    let cursor = term.renderable_content().cursor;
    let alacritty_shape = term.cursor_style().shape;
    let visible = term.mode().contains(TermMode::SHOW_CURSOR)
        && alacritty_shape != AlacrittyCursorShape::Hidden;
    let shape = match alacritty_shape {
        AlacrittyCursorShape::Hidden => CursorShape::Block,
        AlacrittyCursorShape::Beam => CursorShape::Beam,
        AlacrittyCursorShape::Underline => CursorShape::Underline,
        AlacrittyCursorShape::Block | AlacrittyCursorShape::HollowBlock => CursorShape::Block,
    };

    CursorState::with_shape(
        u16::try_from(cursor.point.column.0).expect("terminal columns are bounded to 512"),
        u16::try_from(cursor.point.line.0.max(0)).expect("terminal rows are bounded to 512"),
        visible,
        shape,
    )
}

fn terminal_modes(term: &Term<ScreenEvents>) -> TerminalModes {
    let mode = term.mode();
    TerminalModes::new(
        mode.contains(TermMode::APP_CURSOR),
        mode.contains(TermMode::BRACKETED_PASTE),
        mode.contains(TermMode::LINE_WRAP),
    )
    .with_alternate_screen(mode.contains(TermMode::ALT_SCREEN))
}
