//! The clipboard as text (IO-08). A page only gets the clipboard in a paste
//! event, so the ribbon's "Paste game" and "Paste position" buttons read it
//! here, through the clipboard plugin; "Copy game" and "Copy position" write
//! here too, since WebKit only lets a page write during a user gesture,
//! which ends at the first `await`. The frontend has no clipboard
//! permissions.

use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::error::{AppError, AppResult, ErrorCode};

/// The plugin turns arboard's errors into text; this is arboard's
/// `ContentNotAvailable`: the clipboard is empty or holds no text (an image).
fn is_no_text(message: &str) -> bool {
    message.contains("not available in the requested format") || message.contains("clipboard is empty")
}

/// The text on the clipboard, or `None` when it holds none.
#[tauri::command]
pub async fn clipboard_text(app: AppHandle) -> AppResult<Option<String>> {
    match app.clipboard().read_text() {
        Ok(text) => Ok(Some(text)),
        Err(e) if is_no_text(&e.to_string()) => Ok(None),
        Err(e) => Err(AppError::new(
            ErrorCode::Internal,
            format!(
                "Couldn't read the clipboard ({e}). Copy the text again, or press Ctrl+V outside a text box."
            ),
        )),
    }
}

/// Puts `text` on the clipboard.
#[tauri::command]
pub async fn copy_text(app: AppHandle, text: String) -> AppResult<()> {
    app.clipboard().write_text(text).map_err(|e| {
        AppError::new(ErrorCode::Internal, format!("Couldn't copy to the clipboard ({e}). Try again."))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_clipboard_is_no_text_not_an_error() {
        // arboard 3's message for `ContentNotAvailable`.
        let empty =
            "The clipboard contents were not available in the requested format or the clipboard is empty.";
        assert!(is_no_text(empty));
        assert!(!is_no_text("The native clipboard is not accessible due to being held by another party."));
    }
}
