//! TeX as Unicode text (PAR-139): a placeholder until the port of the
//! TUI's renderer (tui/src/markdown/math*.ts) lands; it shows the source.

/// Inline math on one line.
pub fn inline(tex: &str) -> String {
    tex.trim().to_owned()
}

/// Display math as rows of a monospaced grid at most `width` columns
/// wide where it can.
pub fn display(tex: &str, _width: usize) -> Vec<String> {
    tex.trim().lines().map(str::to_owned).collect()
}
