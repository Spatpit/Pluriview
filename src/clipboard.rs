//! Reading text from the Windows clipboard. egui only delivers clipboard text
//! as a paste event for Ctrl+V, so menus that paste read it here.

use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

pub fn text() -> Result<String, &'static str> {
    // CF_UNICODETEXT is always UTF-16 and avoids lossy ANSI URL conversion.
    const CF_UNICODETEXT: u32 = 13;

    unsafe {
        IsClipboardFormatAvailable(CF_UNICODETEXT)
            .map_err(|_| "The clipboard does not contain text.")?;
        OpenClipboard(None).map_err(|_| "The clipboard is temporarily unavailable.")?;

        let result = (|| {
            let handle = GetClipboardData(CF_UNICODETEXT)
                .map_err(|_| "The clipboard text could not be read.")?;
            let memory = HGLOBAL(handle.0);
            let data = GlobalLock(memory);
            if data.is_null() {
                return Err("The clipboard text could not be read.");
            }

            let text = {
                let len = GlobalSize(memory) / std::mem::size_of::<u16>();
                let utf16 = std::slice::from_raw_parts(data.cast::<u16>(), len);
                let nul = utf16.iter().position(|unit| *unit == 0).unwrap_or(len);
                String::from_utf16(&utf16[..nul])
                    .map_err(|_| "The clipboard contains invalid text.")
            };
            let _ = GlobalUnlock(memory);
            text
        })();

        let _ = CloseClipboard();
        result
    }
}
