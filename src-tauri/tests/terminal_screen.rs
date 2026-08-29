use aiterm_lib::remote::model::TerminalSize;
use aiterm_lib::terminal::model::{
    CellAttributes, CursorState, Revision, RowPatch, ScreenApplyError, ScreenCell, ScreenDiff,
    ScreenRow, TerminalColor, TerminalModes,
};

fn row_patch(index: u16, text: &str) -> RowPatch {
    RowPatch::new(index, screen_row(text))
}

fn screen_row(text: &str) -> ScreenRow {
    let mut cells = Vec::new();
    for character in text.chars() {
        let width = if character == '你' { 2 } else { 1 };
        cells.push(
            ScreenCell::try_new(
                character.to_string(),
                width,
                TerminalColor::Default,
                TerminalColor::Default,
                CellAttributes::default(),
            )
            .expect("test glyph width is valid"),
        );
        if width == 2 {
            cells.push(ScreenCell::continuation());
        }
    }
    ScreenRow::try_new(cells, false).expect("test row is well-formed")
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
    let row = ScreenRow::try_new(
        vec![ScreenCell::try_new(
            "x",
            1,
            TerminalColor::Indexed(12),
            TerminalColor::Rgb { r: 1, g: 2, b: 3 },
            CellAttributes::new(true, false, true, false, false, false, false),
        )
        .expect("single-width cell is valid")],
        true,
    )
    .expect("row is well-formed");

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
fn row_construction_rejects_malformed_wide_cells() {
    let wide_lead = ScreenCell::try_new(
        "你",
        2,
        TerminalColor::Default,
        TerminalColor::Default,
        CellAttributes::default(),
    )
    .expect("wide lead cell is valid");

    assert!(ScreenRow::try_new(vec![wide_lead.clone()], false).is_err());
    assert!(ScreenRow::try_new(vec![ScreenCell::continuation()], false).is_err());
    assert!(ScreenRow::try_new(
        vec![
            wide_lead,
            ScreenCell::continuation(),
            ScreenCell::continuation()
        ],
        false,
    )
    .is_err());
    assert!(ScreenCell::try_new(
        "x",
        3,
        TerminalColor::Default,
        TerminalColor::Default,
        CellAttributes::default(),
    )
    .is_err());
}

fn snapshot() -> aiterm_lib::terminal::model::ScreenSnapshot {
    aiterm_lib::terminal::model::ScreenSnapshot::new(
        "tab-1",
        Revision(7),
        TerminalSize::try_new(2, 2).expect("valid terminal size"),
        vec![screen_row("aa"), screen_row("bb")],
        vec![],
        CursorState::new(0, 0, true),
        TerminalModes::new(false, false, true),
    )
}

#[test]
fn snapshot_applies_a_matching_diff_atomically() {
    let mut snapshot = snapshot();
    let modes = TerminalModes::new(true, true, false).with_alternate_screen(true);
    let diff = ScreenDiff::for_tab("tab-1", Revision(7), Revision(8), vec![row_patch(1, "zz")])
        .with_cursor(CursorState::new(1, 1, false))
        .with_modes(modes.clone());

    snapshot.apply(diff).expect("matching diff should apply");

    assert_eq!(snapshot.revision(), Revision(8));
    assert_eq!(snapshot.visible()[1].cells()[0].text(), "z");
    assert_eq!(snapshot.cursor(), &CursorState::new(1, 1, false));
    assert_eq!(snapshot.modes(), &modes);
    assert!(snapshot.modes().alternate_screen());
}

#[test]
fn snapshot_rejects_non_matching_diffs_without_partial_mutation() {
    let cases = [
        (
            ScreenDiff::for_tab("other-tab", Revision(7), Revision(8), vec![]),
            ScreenApplyError::TabIdMismatch,
        ),
        (
            ScreenDiff::for_tab("tab-1", Revision(6), Revision(8), vec![]),
            ScreenApplyError::BaseRevisionMismatch,
        ),
        (
            ScreenDiff::for_tab("tab-1", Revision(7), Revision(7), vec![]),
            ScreenApplyError::RevisionDidNotAdvance,
        ),
        (
            ScreenDiff::for_tab("tab-1", Revision(7), Revision(8), vec![row_patch(2, "x")]),
            ScreenApplyError::RowOutOfBounds,
        ),
        (
            ScreenDiff::for_tab(
                "tab-1",
                Revision(7),
                Revision(8),
                vec![row_patch(0, "x"), row_patch(0, "y")],
            ),
            ScreenApplyError::DuplicateRowPatch,
        ),
    ];

    for (diff, expected_error) in cases {
        let mut snapshot = snapshot();
        let before = snapshot.clone();

        assert_eq!(snapshot.apply(diff), Err(expected_error));
        assert_eq!(snapshot, before);
    }
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
