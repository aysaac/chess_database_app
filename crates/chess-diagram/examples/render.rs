//! Draws a position as an image, to check how diagrams look without the app.
//!
//! cargo run -p chess-diagram --example render -- OUT.png|OUT.svg [FEN] [SIZE] [black]
//!
//! The example adds an arrow, a circled square and a last-move highlight.

use chess_core::game::{Shape, ShapeColor};
use chess_core::position;
use chess_core::shakmaty::{Color, Position, Square};
use chess_diagram::Diagram;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let out = args.next().ok_or("usage: render OUT.png|OUT.svg [FEN] [SIZE] [black]")?;
    let fen = args
        .next()
        .unwrap_or_else(|| "r1bqkbnr/pppp1ppp/2n5/1B2p3/4P3/5N2/PPPP1PPP/RNBQK2R b KQkq - 3 3".to_owned());
    let pos = position::from_fen(&fen)?;
    let mut d = Diagram::new(pos.board().clone());
    if let Some(size) = args.next() {
        d.size = size.parse()?;
    }
    if args.next().as_deref() == Some("black") {
        d.orientation = Color::Black;
    }
    d.check = if pos.is_check() { pos.board().king_of(pos.turn()) } else { None };
    d.last_move = Some((Square::F1, Square::B5));
    d.shapes = vec![
        Shape { color: ShapeColor::Green, from: Square::A7, to: Some(Square::A6) },
        Shape { color: ShapeColor::Red, from: Square::C6, to: None },
    ];
    d.title = Some(fen);
    if out.ends_with(".svg") {
        std::fs::write(&out, d.to_svg()?)?;
    } else {
        std::fs::write(&out, d.to_png()?)?;
    }
    println!("wrote {out}");
    Ok(())
}
