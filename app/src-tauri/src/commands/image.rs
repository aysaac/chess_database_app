//! Board images (QOL-06): the position on screen as a PNG or SVG, copied to
//! the clipboard or saved to a file. Drawn by `chess-diagram`.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use chess_core::position;
use chess_core::shakmaty::{Color, Position};
use chess_diagram::Diagram;
use tauri::image::Image;
use tauri::{AppHandle, State};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::dto::{BoardImageDto, ImageFormatDto};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::{AppState, Session};

/// The diagram of the session's position at the cursor, with its last move,
/// check and drawings, looking like `image` says.
fn diagram(session: &Session, image: &BoardImageDto) -> AppResult<Diagram> {
    let game = session.editor.game();
    let cursor = session.editor.cursor();
    let pos = game.position_at(cursor);
    let node = game.node(cursor);
    let mut d = Diagram::new(pos.board().clone());
    d.size = image.size;
    d.orientation = match image.orientation.as_str() {
        "white" => Color::White,
        "black" => Color::Black,
        other => return Err(AppError::invalid(format!("bad orientation {other:?}"))),
    };
    d.light = image.light.parse()?;
    d.dark = image.dark.parse()?;
    d.coordinates = image.coordinates;
    if image.last_move {
        // As on the board: castling goes from the king to its target square.
        d.last_move =
            node.mv.and_then(|m| Some((m.from()?, position::castle_king_target(m).unwrap_or(m.to()))));
    }
    d.check = if pos.is_check() { pos.board().king_of(pos.turn()) } else { None };
    d.shapes = node.shapes.clone();
    d.title = Some(position::to_fen(&pos));
    Ok(d)
}

/// Copies the board as an image (PNG) or as SVG markup (text).
#[tauri::command]
pub async fn copy_board_image(
    app: AppHandle,
    state: State<'_, AppState>,
    session: u32,
    image: BoardImageDto,
    format: ImageFormatDto,
) -> AppResult<()> {
    let d = state.with_session(session, |s| diagram(s, &image))?;
    let written = match format {
        ImageFormatDto::Png => {
            let rgba = d.to_rgba()?;
            app.clipboard().write_image(&Image::new_owned(rgba.data, rgba.width, rgba.height))
        }
        ImageFormatDto::Svg => app.clipboard().write_text(d.to_svg()?),
    };
    written.map_err(|e| {
        AppError::new(
            ErrorCode::Internal,
            format!("Couldn't copy to the clipboard ({e}). Try again, or save the image as a file."),
        )
    })
}

/// Saves the board image to `path`: SVG for a `.svg` file, PNG otherwise.
#[tauri::command]
pub async fn save_board_image(
    state: State<'_, AppState>,
    session: u32,
    image: BoardImageDto,
    path: String,
) -> AppResult<()> {
    let d = state.with_session(session, |s| diagram(s, &image))?;
    let path = PathBuf::from(path);
    let svg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    let bytes = if svg { d.to_svg()?.into_bytes() } else { d.to_png()? };
    write_file(&path, &bytes).map_err(|e| {
        AppError::new(
            ErrorCode::Io,
            format!("Couldn't save {} ({e}). Choose another folder or file name.", path.display()),
        )
    })
}

/// Writes a whole file through a temporary file and a rename, so a failed
/// write never leaves half an image under the chosen name.
fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);
    let written = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()
        // The file is closed here, before the rename (Windows).
    })();
    let result = written.and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::game::{GameEditor, Shape, ShapeColor};
    use chess_core::pgn::read_pgn_str;
    use chess_core::shakmaty::Square;

    fn look(orientation: &str) -> BoardImageDto {
        BoardImageDto {
            size: 240,
            orientation: orientation.into(),
            light: "#eeeeee".into(),
            dark: "#556b2f".into(),
            coordinates: true,
            last_move: true,
        }
    }

    fn session(pgn: &str) -> Session {
        let mut s = Session::new(GameEditor::new(read_pgn_str(pgn).remove(0)), None);
        s.editor.to_end();
        s
    }

    #[test]
    fn diagram_shows_the_cursor_position() {
        let s = session("1. e4 e5 2. Nf3 Nc6 3. Bc4 Nf6 4. O-O Nxe4 5. Bxf7+ {[%cal Ge1f1][%csl Rg8]} *");
        let d = diagram(&s, &look("black")).unwrap();
        assert_eq!(d.orientation, Color::Black);
        assert_eq!(d.size, 240);
        assert_eq!(d.last_move, Some((Square::C4, Square::F7)));
        assert_eq!(d.check, Some(Square::E8));
        assert_eq!(d.shapes.len(), 2);
        assert_eq!(d.shapes[1], Shape { color: ShapeColor::Red, from: Square::G8, to: None });
        assert!(d.title.as_deref().unwrap().starts_with("r1bqkb1r/pppp1Bpp/"));

        let mut plain = look("white");
        plain.last_move = false;
        assert_eq!(diagram(&s, &plain).unwrap().last_move, None);
    }

    #[test]
    fn castling_highlights_the_kings_move() {
        let s = session("1. e4 e5 2. Nf3 Nc6 3. Bc4 Nf6 4. O-O *");
        assert_eq!(diagram(&s, &look("white")).unwrap().last_move, Some((Square::E1, Square::G1)));
    }

    #[test]
    fn bad_looks_are_refused() {
        let s = session("1. e4 *");
        let mut bad = look("sideways");
        assert!(diagram(&s, &bad).is_err());
        bad = look("white");
        bad.light = "beige".into();
        let e = diagram(&s, &bad).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn files_are_written_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.png");
        write_file(&path, b"first").unwrap();
        write_file(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no .part left");
        assert!(write_file(&dir.path().join("missing").join("board.png"), b"x").is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
