use crate::remote::model::TerminalSize;
use crate::terminal::MAX_SCROLLBACK_ROWS;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalColor {
    Default,
    Indexed(u8),
    Rgb { r: u8, g: u8, b: u8 },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellAttributes {
    bold: bool,
    faint: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
    hidden: bool,
    strikethrough: bool,
}

impl CellAttributes {
    pub fn new(
        bold: bool,
        faint: bool,
        italic: bool,
        underline: bool,
        inverse: bool,
        hidden: bool,
        strikethrough: bool,
    ) -> Self {
        Self {
            bold,
            faint,
            italic,
            underline,
            inverse,
            hidden,
            strikethrough,
        }
    }

    pub fn bold(&self) -> bool {
        self.bold
    }

    pub fn faint(&self) -> bool {
        self.faint
    }

    pub fn italic(&self) -> bool {
        self.italic
    }

    pub fn underline(&self) -> bool {
        self.underline
    }

    pub fn inverse(&self) -> bool {
        self.inverse
    }

    pub fn hidden(&self) -> bool {
        self.hidden
    }

    pub fn strikethrough(&self) -> bool {
        self.strikethrough
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenCell {
    text: String,
    width: u8,
    continuation: bool,
    foreground: TerminalColor,
    background: TerminalColor,
    attributes: CellAttributes,
}

impl ScreenCell {
    pub fn new(
        text: impl Into<String>,
        width: u8,
        foreground: TerminalColor,
        background: TerminalColor,
        attributes: CellAttributes,
    ) -> Self {
        Self {
            text: text.into(),
            width,
            continuation: false,
            foreground,
            background,
            attributes,
        }
    }

    pub fn continuation() -> Self {
        Self {
            text: String::new(),
            width: 0,
            continuation: true,
            foreground: TerminalColor::Default,
            background: TerminalColor::Default,
            attributes: CellAttributes::default(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn width(&self) -> u8 {
        self.width
    }

    pub fn is_continuation(&self) -> bool {
        self.continuation
    }

    pub fn foreground(&self) -> &TerminalColor {
        &self.foreground
    }

    pub fn background(&self) -> &TerminalColor {
        &self.background
    }

    pub fn attributes(&self) -> &CellAttributes {
        &self.attributes
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRow {
    cells: Vec<ScreenCell>,
    wrapped: bool,
}

impl ScreenRow {
    pub fn new(cells: Vec<ScreenCell>, wrapped: bool) -> Self {
        Self { cells, wrapped }
    }

    pub fn cells(&self) -> &[ScreenCell] {
        &self.cells
    }

    pub fn wrapped(&self) -> bool {
        self.wrapped
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorShape {
    Block,
    Beam,
    Underline,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorState {
    col: u16,
    row: u16,
    visible: bool,
    shape: CursorShape,
}

impl CursorState {
    pub fn new(col: u16, row: u16, visible: bool) -> Self {
        Self::with_shape(col, row, visible, CursorShape::Block)
    }

    pub fn with_shape(col: u16, row: u16, visible: bool, shape: CursorShape) -> Self {
        Self {
            col,
            row,
            visible,
            shape,
        }
    }

    pub fn col(&self) -> u16 {
        self.col
    }

    pub fn row(&self) -> u16 {
        self.row
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn shape(&self) -> CursorShape {
        self.shape
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalModes {
    application_cursor: bool,
    bracketed_paste: bool,
    line_wrap: bool,
}

impl TerminalModes {
    pub fn new(application_cursor: bool, bracketed_paste: bool, line_wrap: bool) -> Self {
        Self {
            application_cursor,
            bracketed_paste,
            line_wrap,
        }
    }

    pub fn application_cursor(&self) -> bool {
        self.application_cursor
    }

    pub fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    pub fn line_wrap(&self) -> bool {
        self.line_wrap
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenSnapshot {
    tab_id: String,
    revision: Revision,
    cols: u16,
    rows: u16,
    visible: Vec<ScreenRow>,
    scrollback: Vec<ScreenRow>,
    cursor: CursorState,
    modes: TerminalModes,
}

impl ScreenSnapshot {
    pub fn new(
        tab_id: impl Into<String>,
        revision: Revision,
        size: TerminalSize,
        visible: Vec<ScreenRow>,
        mut scrollback: Vec<ScreenRow>,
        cursor: CursorState,
        modes: TerminalModes,
    ) -> Self {
        if scrollback.len() > MAX_SCROLLBACK_ROWS {
            let first = scrollback.len() - MAX_SCROLLBACK_ROWS;
            scrollback.drain(..first);
        }
        Self {
            tab_id: tab_id.into(),
            revision,
            cols: size.cols(),
            rows: size.rows(),
            visible,
            scrollback,
            cursor,
            modes,
        }
    }

    pub fn tab_id(&self) -> &str {
        &self.tab_id
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn visible(&self) -> &[ScreenRow] {
        &self.visible
    }

    pub fn scrollback(&self) -> &[ScreenRow] {
        &self.scrollback
    }

    pub fn cursor(&self) -> &CursorState {
        &self.cursor
    }

    pub fn modes(&self) -> &TerminalModes {
        &self.modes
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowPatch {
    row: u16,
    content: ScreenRow,
}

impl RowPatch {
    pub fn new(row: u16, content: ScreenRow) -> Self {
        Self { row, content }
    }

    pub fn row(&self) -> u16 {
        self.row
    }

    pub fn content(&self) -> &ScreenRow {
        &self.content
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenDiff {
    tab_id: String,
    base_revision: Revision,
    revision: Revision,
    rows: Vec<RowPatch>,
    cursor: Option<CursorState>,
    modes: Option<TerminalModes>,
}

impl ScreenDiff {
    pub fn new(base_revision: Revision, revision: Revision, rows: Vec<RowPatch>) -> Self {
        Self::for_tab(String::new(), base_revision, revision, rows)
    }

    pub fn for_tab(
        tab_id: impl Into<String>,
        base_revision: Revision,
        revision: Revision,
        rows: Vec<RowPatch>,
    ) -> Self {
        Self {
            tab_id: tab_id.into(),
            base_revision,
            revision,
            rows,
            cursor: None,
            modes: None,
        }
    }

    pub fn with_cursor(mut self, cursor: CursorState) -> Self {
        self.cursor = Some(cursor);
        self
    }

    pub fn with_modes(mut self, modes: TerminalModes) -> Self {
        self.modes = Some(modes);
        self
    }

    pub fn tab_id(&self) -> &str {
        &self.tab_id
    }

    pub fn base_revision(&self) -> Revision {
        self.base_revision
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    pub fn rows(&self) -> &[RowPatch] {
        &self.rows
    }

    pub fn cursor(&self) -> Option<&CursorState> {
        self.cursor.as_ref()
    }

    pub fn modes(&self) -> Option<&TerminalModes> {
        self.modes.as_ref()
    }
}
