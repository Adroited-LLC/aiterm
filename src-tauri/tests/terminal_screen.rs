use aiterm_lib::remote::model::{encode_terminal_frame, TerminalSize};
use aiterm_lib::terminal::model::{
    CellAttributes, CursorState, Revision, RowPatch, ScreenApplyError, ScreenCell, ScreenDiff,
    ScreenRow, ScreenSnapshot, TerminalColor, TerminalModes,
};
use aiterm_lib::terminal::screen::ScreenModel;

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

fn size(cols: u16, rows: u16) -> TerminalSize {
    TerminalSize::try_new(cols, rows).expect("test terminal dimensions are valid")
}

fn tab() -> &'static str {
    "tab-1"
}

fn row_text(row: &ScreenRow) -> String {
    row.cells()
        .iter()
        .filter(|cell| !cell.is_continuation())
        .map(ScreenCell::text)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

fn visible_text(snapshot: &ScreenSnapshot) -> Vec<String> {
    snapshot.visible().iter().map(row_text).collect()
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
fn snapshot_rejects_rows_and_cursor_outside_its_applied_dimensions_atomically() {
    let too_wide =
        ScreenDiff::for_tab("tab-1", Revision(7), Revision(8), vec![row_patch(0, "xxx")]);
    let cursor_outside = ScreenDiff::for_tab("tab-1", Revision(7), Revision(8), vec![])
        .with_cursor(CursorState::new(2, 0, true));

    for (diff, expected) in [
        (too_wide, ScreenApplyError::RowTooWide),
        (cursor_outside, ScreenApplyError::CursorOutOfBounds),
    ] {
        let mut snapshot = snapshot();
        let before = snapshot.clone();
        assert_eq!(snapshot.apply(diff), Err(expected));
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

#[test]
fn screen_model_split_utf8_and_combining_marks_survive_a_snapshot() {
    let mut screen = ScreenModel::new(size(8, 2));
    screen.process(&[0xe4, 0xbd]);
    screen.process(&[0xa0, b'e', 0xcc, 0x81]);

    assert_eq!(
        visible_text(&screen.snapshot(tab())),
        vec!["你e\u{301}", ""]
    );
}

#[test]
fn screen_model_alternate_screen_is_current_even_when_it_started_before_attach() {
    let mut screen = ScreenModel::new(size(12, 3));
    screen.process(b"shell\r\n\x1b[?1049h\x1b[Hfull screen");
    let snapshot = screen.snapshot(tab());

    assert!(snapshot.modes().alternate_screen());
    assert!(visible_text(&snapshot)[0].contains("full screen"));
}

#[test]
fn screen_model_damage_applied_to_previous_snapshot_equals_a_fresh_snapshot() {
    let mut screen = ScreenModel::new(size(20, 4));
    screen.process(b"before");
    let mut client = screen.snapshot(tab());
    let damage = screen.process(b"\r\x1b[32mafter\x1b[0m");

    client
        .apply(damage.diff.expect("visible changes produce a diff"))
        .expect("adapter diffs apply to the last snapshot");
    assert_eq!(client, screen.snapshot(tab()));
}

#[test]
fn screen_model_scroll_damage_applied_to_a_client_equals_a_fresh_viewport() {
    let mut screen = ScreenModel::new(size(8, 2));
    screen.process(b"one\r\ntwo");
    let mut client = screen.snapshot(tab());

    let damage = screen.process(b"\r\nthree");
    client
        .apply(damage.diff.expect("scrolling damages the viewport"))
        .expect("scroll damage follows the client's revision");

    assert_eq!(client, screen.snapshot(tab()));
    assert!(client.scrollback().is_empty());
    assert_eq!(visible_text(&client), vec!["two", "three"]);
    assert_eq!(
        screen
            .scrollback_page(0, 1)
            .iter()
            .map(row_text)
            .collect::<Vec<_>>(),
        vec!["one"]
    );
}

#[test]
fn screen_model_preserves_rgb_and_indexed_colors() {
    let mut screen = ScreenModel::new(size(8, 2));
    screen.process(b"\x1b[38;2;1;2;3mR\x1b[0m\x1b[48;5;42mI");
    let snapshot = screen.snapshot(tab());
    let cells = snapshot.visible()[0].cells();

    assert_eq!(
        cells[0].foreground(),
        &TerminalColor::Rgb { r: 1, g: 2, b: 3 }
    );
    assert_eq!(cells[1].background(), &TerminalColor::Indexed(42));
}

#[test]
fn screen_model_maps_adapter_level_attribute_flags() {
    let mut screen = ScreenModel::new(size(8, 2));
    screen.process(b"\x1b[1;2;3;4;7;8;9mX");
    let snapshot = screen.snapshot(tab());
    let attributes = snapshot.visible()[0].cells()[0].attributes();

    assert!(attributes.bold());
    assert!(attributes.faint());
    assert!(attributes.italic());
    assert!(attributes.underline());
    assert!(attributes.inverse());
    assert!(attributes.hidden());
    assert!(attributes.strikethrough());
}

#[test]
fn screen_model_reports_cursor_position_shape_and_visibility() {
    let mut screen = ScreenModel::new(size(8, 3));
    screen.process(b"\x1b[2;4H\x1b[6 q\x1b[?25l");
    let snapshot = screen.snapshot(tab());

    assert_eq!(snapshot.cursor().row(), 1);
    assert_eq!(snapshot.cursor().col(), 3);
    assert_eq!(
        snapshot.cursor().shape(),
        aiterm_lib::terminal::model::CursorShape::Beam
    );
    assert!(!snapshot.cursor().visible());
}

#[test]
fn screen_model_wide_cells_have_an_explicit_continuation() {
    let mut screen = ScreenModel::new(size(8, 2));
    screen.process("你".as_bytes());
    let snapshot = screen.snapshot(tab());
    let cells = snapshot.visible()[0].cells();

    assert_eq!(cells[0].text(), "你");
    assert_eq!(cells[0].width(), 2);
    assert!(cells[1].is_continuation());
}

#[test]
fn screen_model_marks_rows_wrapped_by_the_terminal() {
    let mut screen = ScreenModel::new(size(4, 2));
    screen.process(b"abcde");
    let snapshot = screen.snapshot(tab());

    assert_eq!(visible_text(&snapshot), vec!["abcd", "e"]);
    assert!(snapshot.visible()[0].wrapped());
    assert!(!snapshot.visible()[1].wrapped());
}

#[test]
fn screen_model_resize_reflows_wrapped_content() {
    let mut screen = ScreenModel::new(size(6, 2));
    screen.process(b"abcdefghi");
    screen.resize(size(4, 3));
    let snapshot = screen.snapshot(tab());

    assert_eq!((snapshot.cols(), snapshot.rows()), (4, 3));
    assert_eq!(visible_text(&snapshot), vec!["efgh", "i", ""]);
    assert!(snapshot.scrollback().is_empty());
    let history = screen.scrollback_page(0, 1);
    assert_eq!(
        history.iter().map(row_text).collect::<Vec<_>>(),
        vec!["abcd"]
    );
    assert!(history[0].wrapped());
    assert!(snapshot.visible()[0].wrapped());
}

#[test]
fn screen_model_emits_title_and_bell_metadata() {
    let mut screen = ScreenModel::new(size(8, 2));
    let damage = screen.process(b"\x1b]2;build log\x07\x07");

    assert_eq!(damage.title.as_deref(), Some("build log"));
    assert!(damage.bell);
}

#[test]
fn screen_model_reset_title_is_an_explicit_empty_title_update() {
    let mut screen = ScreenModel::new(size(8, 2));
    screen.process(b"\x1b[22;0t\x1b]2;temporary\x07");
    let damage = screen.process(b"\x1b[23;0t");

    assert_eq!(damage.title.as_deref(), Some(""));
}

#[test]
fn screen_model_rejects_clipboard_events_without_leaking_a_reply() {
    let mut screen = ScreenModel::new(size(8, 2));
    let clipboard = screen.process(b"\x1b]52;c;c2VjcmV0\x07");

    assert!(clipboard.replies.is_empty());
    assert!(clipboard.title.is_none());
    assert!(!clipboard.bell);

    let query = screen.process(b"\x1b[c");
    assert_eq!(query.replies, vec![b"\x1b[?6c".to_vec()]);
}

#[test]
fn screen_model_tracks_bracketed_paste_and_application_cursor_modes() {
    let mut screen = ScreenModel::new(size(8, 2));
    screen.process(b"\x1b[?2004h\x1b[?1h");
    let snapshot = screen.snapshot(tab());

    assert!(snapshot.modes().bracketed_paste());
    assert!(snapshot.modes().application_cursor());
    assert!(snapshot.modes().line_wrap());
}

#[test]
fn screen_model_pages_scrollback_from_the_newest_history_row() {
    let mut screen = ScreenModel::new(size(8, 2));
    screen.process(b"one\r\ntwo\r\nthree\r\nfour");

    let first_page = screen.scrollback_page(0, 2);
    let next_page = screen.scrollback_page(2, 2);
    assert_eq!(
        first_page.iter().map(row_text).collect::<Vec<_>>(),
        vec!["two", "one"]
    );
    assert!(next_page.is_empty());
    assert!(screen.scrollback_page(0, 10_000).len() <= 5_000);
}

#[test]
fn screen_model_clamps_scrollback_requests_to_five_thousand_rows() {
    let mut screen = ScreenModel::new(size(8, 2));
    let mut output = Vec::with_capacity(5_100 * 3);
    for _ in 0..5_100 {
        output.extend_from_slice(b"x\r\n");
    }
    screen.process(&output);

    assert_eq!(screen.scrollback_page(0, 10_000).len(), 5_000);
    assert!(screen.scrollback_page(5_000, 1).is_empty());
}

#[test]
fn screen_model_collects_terminal_query_replies() {
    let mut screen = ScreenModel::new(size(8, 2));
    let damage = screen.process(b"\x1b[c\x1b[6n");

    assert!(damage.replies.iter().any(|reply| reply == b"\x1b[?6c"));
    assert!(damage.replies.iter().any(|reply| reply == b"\x1b[1;1R"));
}

#[test]
fn screen_model_only_answers_queries_with_adapter_owned_data() {
    let mut screen = ScreenModel::new(size(8, 2));
    let damage = screen.process(b"\x1b]10;?\x07\x1b[14t\x1b[18t\x1b[c");

    assert_eq!(
        damage.replies,
        vec![b"\x1b[8;2;8t".to_vec(), b"\x1b[?6c".to_vec()]
    );
}

#[test]
fn screen_model_bounds_combining_content_so_one_max_width_row_encodes() {
    let mut screen = ScreenModel::new(size(512, 1));
    let _client = screen.snapshot(tab());
    let mut output = String::with_capacity(512 * 4_097);
    for _ in 0..512 {
        output.push('x');
        for _ in 0..2_048 {
            output.push('\u{e0100}');
        }
    }

    let damage = screen.process(output.as_bytes());
    let diff = damage.diff.expect("the populated row is damaged");
    assert!(diff
        .rows()
        .iter()
        .flat_map(|patch| patch.content().cells())
        .all(|cell| cell.text().chars().count() <= 33));
    encode_terminal_frame(&diff).expect("a row-granular diff must fit one typed frame");
}

#[test]
fn screen_model_resize_advances_revision_and_requires_snapshot_recovery() {
    let mut screen = ScreenModel::new(size(8, 2));
    screen.process(b"before");
    let before = screen.snapshot(tab());

    let damage = screen.resize(size(4, 3));
    assert!(damage.diff.is_none());
    let mut recovered = screen.snapshot(tab());
    assert!(recovered.revision().0 > before.revision().0);
    assert_eq!((recovered.cols(), recovered.rows()), (4, 3));

    let damage = screen.process(b"after");
    recovered
        .apply(damage.diff.expect("post-resize output produces a diff"))
        .expect("post-resize diff follows the recovery snapshot");
    assert_eq!(recovered, screen.snapshot(tab()));
}

#[test]
fn screen_model_handles_ten_thousand_deterministic_byte_and_resize_operations() {
    let mut screen = ScreenModel::new(size(80, 24));
    let mut client = screen.snapshot(tab());
    let mut state = 0x4d59_5df4_d0f3_3173_u64;
    let mut encoded_single_row_diffs = 0;

    for operation in 0..10_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        if operation % 97 == 0 {
            let cols = ((state & 0x1ff) as u16).clamp(1, 512);
            let rows = (((state >> 9) & 0x1ff) as u16).clamp(1, 512);
            let damage = screen.resize(size(cols, rows));
            assert!(damage.diff.is_none());
            client = screen.snapshot(tab());
        } else {
            let damage = screen.process(&[(state & 0xff) as u8]);
            if let Some(diff) = damage.diff {
                if diff.rows().len() == 1 {
                    encode_terminal_frame(&diff)
                        .expect("a row-granular ordinary diff must fit one typed frame");
                    encoded_single_row_diffs += 1;
                }
                client
                    .apply(diff)
                    .expect("live damage follows the client revision");
            }
        }

        if operation % 211 == 0 {
            assert_eq!(client, screen.snapshot(tab()));
        }
    }

    let snapshot = screen.snapshot(tab());
    assert_eq!(client, snapshot);
    assert!((1..=512).contains(&snapshot.cols()));
    assert!((1..=512).contains(&snapshot.rows()));
    assert!(snapshot.scrollback().is_empty());
    assert!(encoded_single_row_diffs > 0);
}

#[test]
fn screen_model_can_be_owned_by_the_synchronized_tab_registry() {
    fn assert_send<T: Send>() {}

    assert_send::<ScreenModel>();
}
