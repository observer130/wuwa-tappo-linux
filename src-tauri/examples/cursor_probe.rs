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
            .map(|r| (r.cursor_serial, r.width, r.height, r.xhot, r.yhot));
        let pointer = conn.query_pointer(root).ok().and_then(|c| c.reply().ok());
        if let (Some((serial, w, h, xh, yh)), Some(p)) = (snap, pointer) {
            if Some((serial, w, h, xh, yh)) != last {
                println!(
                    "cursor serial={} {}x{} hot=({},{}) at ({},{})",
                    serial, w, h, xh, yh, p.root_x, p.root_y
                );
                last = Some((serial, w, h, xh, yh));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
