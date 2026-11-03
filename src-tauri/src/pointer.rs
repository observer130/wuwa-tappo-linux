//! X11 pointer sampling for global input.
//!
//! Tao/GDK's `cursor_position()` is not reliable when called from a worker
//! thread: it can return the position captured at startup instead of the live
//! pointer. The application runs under X11/XWayland, so query the root window
//! through a dedicated XCB connection owned by that worker thread.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::rust_connection::RustConnection;

pub struct PointerReader {
    connection: RustConnection,
    root: u32,
}

impl PointerReader {
    pub fn new() -> Option<Self> {
        let (connection, screen) = x11rb::connect(None).ok()?;
        let root = connection.setup().roots.get(screen)?.root;
        Some(Self { connection, root })
    }

    pub fn position(&self) -> Option<(f64, f64)> {
        let reply = self
            .connection
            .query_pointer(self.root)
            .ok()?
            .reply()
            .ok()?;
        Some((reply.root_x as f64, reply.root_y as f64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_x11_pointer_when_display_is_available() {
        if std::env::var_os("DISPLAY").is_none() {
            return;
        }
        let reader = PointerReader::new().expect("X11 pointer connection");
        let (x, y) = reader.position().expect("pointer position");
        assert!(x.is_finite() && y.is_finite());
    }
}
