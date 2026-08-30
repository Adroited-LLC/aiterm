use crate::remote::model::TerminalSize;
use crate::terminal::MAX_SCROLLBACK_ROWS;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use std::collections::HashSet;
use unicode_normalization::char::is_combining_mark;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ScreenCell {
    text: String,
    width: u8,
    continuation: bool,
    foreground: TerminalColor,
    background: TerminalColor,
    attributes: CellAttributes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenCellError {
    InvalidWidth,
}

impl std::fmt::Display for ScreenCellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("screen cell width must be one or two")
    }
}

impl std::error::Error for ScreenCellError {}

impl ScreenCell {
    pub fn try_new(
        text: impl Into<String>,
        width: u8,
        foreground: TerminalColor,
        background: TerminalColor,
        attributes: CellAttributes,
    ) -> Result<Self, ScreenCellError> {
        if !matches!(width, 1 | 2) {
            return Err(ScreenCellError::InvalidWidth);
        }
        Ok(Self {
            text: text.into(),
            width,
            continuation: false,
            foreground,
            background,
            attributes,
        })
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

#[derive(Deserialize)]
struct ScreenCellWire {
    text: String,
    width: u8,
    continuation: bool,
    foreground: TerminalColor,
    background: TerminalColor,
    attributes: CellAttributes,
}

impl<'de> Deserialize<'de> for ScreenCell {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ScreenCellWire::deserialize(deserializer)?;
        if wire.continuation {
            if wire.width != 0
                || !wire.text.is_empty()
                || wire.foreground != TerminalColor::Default
                || wire.background != TerminalColor::Default
                || wire.attributes != CellAttributes::default()
            {
                return Err(D::Error::custom("invalid continuation cell"));
            }
            return Ok(Self::continuation());
        }
        Self::try_new(
            wire.text,
            wire.width,
            wire.foreground,
            wire.background,
            wire.attributes,
        )
        .map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ScreenRow {
    cells: Vec<ScreenCell>,
    wrapped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenRowError {
    OrphanContinuation,
    WideCellMissingContinuation,
}

impl std::fmt::Display for ScreenRowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OrphanContinuation => f.write_str("wide-cell continuation has no lead cell"),
            Self::WideCellMissingContinuation => {
                f.write_str("wide-cell lead must be followed by one continuation")
            }
        }
    }
}

impl std::error::Error for ScreenRowError {}

impl ScreenRow {
    pub fn try_new(cells: Vec<ScreenCell>, wrapped: bool) -> Result<Self, ScreenRowError> {
        let mut index = 0;
        while index < cells.len() {
            let cell = &cells[index];
            if cell.is_continuation() {
                return Err(ScreenRowError::OrphanContinuation);
            }
            if cell.width() == 2 {
                let continuation = cells.get(index + 1);
                if !continuation.is_some_and(ScreenCell::is_continuation) {
                    return Err(ScreenRowError::WideCellMissingContinuation);
                }
                index += 2;
            } else {
                index += 1;
            }
        }
        Ok(Self { cells, wrapped })
    }

    pub fn cells(&self) -> &[ScreenCell] {
        &self.cells
    }

    pub fn wrapped(&self) -> bool {
        self.wrapped
    }

    pub(crate) fn has_valid_cell_text(&self) -> bool {
        self.cells.iter().all(|cell| {
            if cell.is_continuation() {
                return true;
            }
            let mut scalars = cell.text().chars();
            let Some(base) = scalars.next() else {
                return false;
            };
            if is_combining_mark(base) {
                return false;
            }
            scalars
                .take(33)
                .enumerate()
                .all(|(index, scalar)| index < 32 && is_combining_mark(scalar))
        })
    }
}

#[derive(Deserialize)]
struct ScreenRowWire {
    cells: Vec<ScreenCell>,
    wrapped: bool,
}

impl<'de> Deserialize<'de> for ScreenRow {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ScreenRowWire::deserialize(deserializer)?;
        Self::try_new(wire.cells, wire.wrapped).map_err(D::Error::custom)
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
    alternate_screen: bool,
}

impl TerminalModes {
    pub fn new(application_cursor: bool, bracketed_paste: bool, line_wrap: bool) -> Self {
        Self {
            application_cursor,
            bracketed_paste,
            line_wrap,
            alternate_screen: false,
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

    pub fn with_alternate_screen(mut self, alternate_screen: bool) -> Self {
        self.alternate_screen = alternate_screen;
        self
    }

    pub fn alternate_screen(&self) -> bool {
        self.alternate_screen
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

    /// Apply a diff only when it is for this tab and exactly follows this
    /// snapshot. Validation completes before any snapshot field is changed.
    pub fn apply(&mut self, diff: ScreenDiff) -> Result<(), ScreenApplyError> {
        if diff.tab_id != self.tab_id {
            return Err(ScreenApplyError::TabIdMismatch);
        }
        if diff.base_revision != self.revision {
            return Err(ScreenApplyError::BaseRevisionMismatch);
        }
        if diff.revision.0 <= self.revision.0 {
            return Err(ScreenApplyError::RevisionDidNotAdvance);
        }

        let mut patched_rows = HashSet::with_capacity(diff.rows.len());
        for patch in &diff.rows {
            let row = usize::from(patch.row);
            if row >= usize::from(self.rows) || row >= self.visible.len() {
                return Err(ScreenApplyError::RowOutOfBounds);
            }
            if !patched_rows.insert(row) {
                return Err(ScreenApplyError::DuplicateRowPatch);
            }
            if patch.content.cells().len() > usize::from(self.cols) {
                return Err(ScreenApplyError::RowTooWide);
            }
        }
        if diff
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.col() >= self.cols || cursor.row() >= self.rows)
        {
            return Err(ScreenApplyError::CursorOutOfBounds);
        }

        for patch in diff.rows {
            self.visible[usize::from(patch.row)] = patch.content;
        }
        if let Some(cursor) = diff.cursor {
            self.cursor = cursor;
        }
        if let Some(modes) = diff.modes {
            self.modes = modes;
        }
        self.revision = diff.revision;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenApplyError {
    TabIdMismatch,
    BaseRevisionMismatch,
    RevisionDidNotAdvance,
    RowOutOfBounds,
    DuplicateRowPatch,
    RowTooWide,
    CursorOutOfBounds,
}

impl std::fmt::Display for ScreenApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TabIdMismatch => f.write_str("diff tab id does not match snapshot"),
            Self::BaseRevisionMismatch => f.write_str("diff base revision does not match snapshot"),
            Self::RevisionDidNotAdvance => f.write_str("diff revision must advance the snapshot"),
            Self::RowOutOfBounds => f.write_str("diff row is outside the visible screen"),
            Self::DuplicateRowPatch => f.write_str("diff contains duplicate row patches"),
            Self::RowTooWide => f.write_str("diff row exceeds the visible screen width"),
            Self::CursorOutOfBounds => f.write_str("diff cursor is outside the visible screen"),
        }
    }
}

impl std::error::Error for ScreenApplyError {}

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
