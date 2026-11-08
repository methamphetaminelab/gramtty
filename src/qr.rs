use qrcode::{Color, QrCode};

const QUIET: usize = 2;

pub fn qr_lines(data: &str) -> Result<Vec<String>, String> {
    let code = QrCode::new(data.as_bytes()).map_err(|e| format!("QR encode failed: {e}"))?;
    let width = code.width();
    let colors = code.to_colors();
    let size = width + QUIET * 2;
    let light = |x: usize, y: usize| -> bool {
        if x < QUIET || y < QUIET || x >= width + QUIET || y >= width + QUIET {
            return true;
        }
        colors[(y - QUIET) * width + (x - QUIET)] == Color::Light
    };
    let mut lines = Vec::with_capacity(size.div_ceil(2));
    for y in (0..size).step_by(2) {
        let mut line = String::with_capacity(size * 3);
        for x in 0..size {
            let top = light(x, y);
            let bottom = y + 1 >= size || light(x, y + 1);
            line.push(match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        lines.push(line);
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_square_with_quiet_zone() {
        let lines = qr_lines("tg://login?token=AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-").unwrap();
        let w = lines[0].chars().count();
        assert_eq!(lines.len(), w.div_ceil(2));
        assert!(lines[0].chars().all(|c| c == '█'));
        assert!(lines.iter().all(|l| l.chars().count() == w));
        assert!(lines.iter().any(|l| l.contains(' ')));
    }
}
