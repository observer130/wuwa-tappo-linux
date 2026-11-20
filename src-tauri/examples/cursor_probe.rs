//! Probe what cursor the X server is actually displaying.
//!
//! XFixes GetCursorImage returns the cursor currently rendered at the
//! pointer position (serial + geometry + pixels). Used as a diagnostic
//! during the v4 cursor fix: compare the cursor serial while the fake
//! pointer hovers the pet vs the desktop.
//!
//! Usage under Xvfb:
//!   cargo run --manifest-path src-tauri/Cargo.toml --release --example cursor_probe

use x11rb::connection::Connection;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::ConnectionExt as _;

fn main() {
    let (conn, screen) = x11rb::connect(None).expect("X connection");
    let root = conn.setup().roots.get(screen).expect("screen").root;
    conn.xfixes_query_version(5, 0)
        .expect("xfixes query version")
        .reply()
        .expect("xfixes version reply");
    let mut last: Option<(u32, u16, u16, u16, u16)> = None;
    loop {
        let snap = conn
            .xfixes_get_cursor_image()
            .map_err(x11rb::errors::ReplyOrIdError::from)
            .and_then(|c| Ok(c.reply()?))
            .ok()
            .map(|r| {
                let meta = (r.cursor_serial, r.width, r.height, r.xhot, r.yhot);
                let bytes: Vec<u8> = r
                    .cursor_image
                    .iter()
                    .flat_map(|px| px.to_le_bytes())
                    .collect();
                (
                    meta,
                    render_ascii(&bytes, r.width as usize, r.height as usize),
                )
            });
        let pointer = conn.query_pointer(root).ok().and_then(|c| c.reply().ok());
        if let (Some(((serial, w, h, xh, yh), art)), Some(p)) = (snap, pointer) {
            if Some((serial, w, h, xh, yh)) != last {
                println!(
                    "cursor serial={} {}x{} hot=({},{}) at ({},{})",
                    serial, w, h, xh, yh, p.root_x, p.root_y
                );
                println!("{art}");
                last = Some((serial, w, h, xh, yh));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Coarse ASCII render of the ARGB cursor image (one char per 2x2 block).
fn render_ascii(image: &[u8], width: usize, height: usize) -> String {
    if width == 0 || height == 0 {
        return String::new();
    }
    let step_y = 2.max(height / 16);
    let step_x = 2.max(width / 16);
    let mut rows = String::new();
    let mut y = 0;
    while y < height {
        let mut line = String::new();
        let mut x = 0;
        while x < width {
            let idx = (y * width + x) * 4;
            let (b, g, r, a) = (
                *image.get(idx).unwrap_or(&0),
                *image.get(idx + 1).unwrap_or(&0),
                *image.get(idx + 2).unwrap_or(&0),
                *image.get(idx + 3).unwrap_or(&0),
            );
            let lum = u32::from(a) * (u32::from(r) + 2 * u32::from(g) + u32::from(b)) / (4 * 255);
            line.push(match (a, lum) {
                (0, _) => '.',
                (_, 0..=40) => '#',
                (_, 41..=90) => '+',
                (_, 91..=160) => 'o',
                _ => '*',
            });
            x += step_x;
        }
        rows.push_str(&line);
        rows.push('\n');
        y += step_y;
    }
    rows
}
