use aiterm_lib::remote::model::TerminalSize;
use aiterm_lib::terminal::model::{
    CellAttributes, CursorState, Revision, RowPatch, ScreenCell, ScreenDiff, ScreenRow,
    TerminalColor, TerminalModes,
};

fn row_patch(index: u16, text: &str) -> RowPatch {
    RowPatch::new(index, screen_row(text))
}

fn screen_row(text: &str) -> ScreenRow {
    let mut cells = Vec::new();
    for character in text.chars() {
        let width = if character == '你' { 2 } else { 1 };
        cells.push(ScreenCell::new(
            character.to_string(),
            width,
            TerminalColor::Default,
            TerminalColor::Default,
            CellAttributes::default(),
        ));
        if width == 2 {
            cells.push(ScreenCell::continuation());
        }
    }
    ScreenRow::new(cells, false)
}

#[test]
fn a_diff_names_the_snapshot_revision_it_applies_to() {
    let diff = ScreenDiff::new(Revision(7), Revision(8), vec![row_patch(3, "ready")]);

    assert_eq!(diff.base_revision(), Revision(7));
    assert_eq!(diff.revision(), Revision(8));
}

#[test]
fn a_wide_glyph_has_one_lead_cell_and_one_explicit_continuation() {
    let row = screen_row("你");

    assert_eq!(row.cells()[0].width(), 2);
    assert!(row.cells()[1].is_continuation());
}

#[test]
fn screen_rows_retain_their_terminal_cell_data() {
    let row = ScreenRow::new(
        vec![ScreenCell::new(
            "x",
            1,
            TerminalColor::Indexed(12),
            TerminalColor::Rgb { r: 1, g: 2, b: 3 },
            CellAttributes::new(true, false, true, false, false, false, false),
        )],
        true,
    );

    assert!(row.wrapped());
    assert_eq!(row.cells()[0].text(), "x");
    assert_eq!(row.cells()[0].foreground(), &TerminalColor::Indexed(12));
    assert_eq!(
        row.cells()[0].background(),
        &TerminalColor::Rgb { r: 1, g: 2, b: 3 }
    );
    assert!(row.cells()[0].attributes().bold());
    assert!(row.cells()[0].attributes().italic());
}

#[test]
fn cursor_and_modes_are_constructed_without_exposing_fields() {
    let cursor = CursorState::new(4, 2, true);
    let modes = TerminalModes::new(true, true, false);
    let size = TerminalSize::try_new(80, 24).expect("valid terminal size");

    assert_eq!(cursor.col(), 4);
    assert_eq!(cursor.row(), 2);
    assert!(cursor.visible());
    assert!(modes.application_cursor());
    assert!(modes.bracketed_paste());
    assert!(!modes.line_wrap());
    assert_eq!(size.cols(), 80);
}
