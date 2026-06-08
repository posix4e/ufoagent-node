//! QR rendering for the link flow: show a scannable code in the console so the operator can
//! approve from a phone or another device — no copy-paste, no browser launch on the node.

use anyhow::Result;

/// Render `data` (the approval URL) as a unicode QR string for a terminal.
///
/// Colors are inverted (QR's dark modules drawn with the *light* glyph) — this is the
/// `qrcode` crate's documented recipe for terminals: on a dark console the printed blocks
/// form a light-on-dark code that phone scanners read. Needs a UTF-8 console (we set
/// `SetConsoleOutputCP(CP_UTF8)` at startup on Windows) and a font with block glyphs.
pub fn unicode(data: &str) -> Result<String> {
    use qrcode::render::unicode;
    use qrcode::QrCode;
    let code = QrCode::new(data.as_bytes())?;
    let rendered = code
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build();
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_nonempty_multiline() {
        let s = unicode("https://app.ufoagent.xyz/link?code=ABCD-1234").unwrap();
        assert!(!s.is_empty());
        // A QR for a URL of this length is well over a handful of rows.
        assert!(s.lines().count() > 5, "expected a multi-line QR, got {s:?}");
    }

    #[test]
    fn errors_on_oversized_input() {
        // Far beyond any QR capacity → the crate returns an error we propagate (not a panic).
        let huge = "x".repeat(10_000);
        assert!(unicode(&huge).is_err());
    }
}
