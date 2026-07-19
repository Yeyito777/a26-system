use std::error::Error;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ChangeGCAux, ConnectionExt as _, Gcontext, Rectangle, Window};

use crate::font;
use crate::metrics::{SystemInfo, usage_text};

pub const BG: u32 = 0x0b1020;
const BG_CARD: u32 = 0x182235;
const FG: u32 = 0xf5f7fb;
const MUTED: u32 = 0x94a3b8;
const ACCENT: u32 = 0x5eead4;
const ACCENT_2: u32 = 0x60a5fa;

pub struct Renderer {
    window: Window,
    gc: Gcontext,
    width: u16,
    height: u16,
}

impl Renderer {
    pub fn new(window: Window, gc: Gcontext, width: u16, height: u16) -> Self {
        Self {
            window,
            gc,
            width,
            height,
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    pub fn render<C: Connection>(
        &self,
        conn: &C,
        system: &SystemInfo,
        volume: Option<u8>,
    ) -> Result<(), Box<dyn Error>> {
        self.fill(
            conn,
            BG,
            Rectangle {
                x: 0,
                y: 0,
                width: self.width,
                height: self.height,
            },
        )?;

        self.text(conn, "SYSTEM", 70, 100, 10, ACCENT)?;
        let volume = volume.map_or_else(|| "--".into(), |value| value.to_string());
        self.text(conn, &format!("VOLUME {volume}"), 70, 260, 5, MUTED)?;

        self.metric_card(
            conn,
            (55, 390),
            "CPU",
            &usage_text(system.cpu_usage),
            ACCENT,
        )?;
        self.metric_card(
            conn,
            (555, 390),
            "GPU",
            &usage_text(system.gpu_usage),
            ACCENT_2,
        )?;
        self.metric_card(conn, (55, 575), "SPACE USED", &system.storage_used, FG)?;
        self.metric_card(conn, (555, 575), "SPACE FREE", &system.storage_free, FG)?;

        self.card_line(conn, 780, "DEVICE", "SAMSUNG GALAXY A26")?;
        self.card_line(conn, 945, "KERNEL", &system.kernel)?;
        self.card_line(conn, 1110, "ROOTFS", &format!("ALPINE {}", system.alpine))?;
        self.card_line(conn, 1275, "MEMORY", &system.memory)?;
        self.card_line(
            conn,
            1440,
            "DISPLAY",
            &format!("{}X{} 120 HZ", self.width, self.height),
        )?;
        self.card_line(conn, 1605, "WINDOW MANAGER", "A26 SHELL RUST")?;
        self.card_line(conn, 1770, "CONTROL", "ADB UNIX SOCKET")?;
        self.center_text(conn, "SWIPE UP FROM BELOW TO CLOSE", 2150, 4, MUTED)?;
        self.home_bar(conn)?;
        conn.flush()?;
        Ok(())
    }

    fn metric_card<C: Connection>(
        &self,
        conn: &C,
        position: (i16, i16),
        noun: &str,
        value: &str,
        color: u32,
    ) -> Result<(), Box<dyn Error>> {
        let (x, y) = position;
        self.fill(
            conn,
            BG_CARD,
            Rectangle {
                x,
                y,
                width: 470,
                height: 145,
            },
        )?;
        self.text(conn, noun, x + 28, y + 24, 3, MUTED)?;
        let clipped: String = value.chars().take(12).collect();
        self.text(conn, &clipped, x + 28, y + 73, 6, color)?;
        Ok(())
    }

    fn card_line<C: Connection>(
        &self,
        conn: &C,
        y: i16,
        noun: &str,
        value: &str,
    ) -> Result<(), Box<dyn Error>> {
        self.fill(
            conn,
            BG_CARD,
            Rectangle {
                x: 55,
                y,
                width: self.width.saturating_sub(110),
                height: 145,
            },
        )?;
        self.text(conn, noun, 85, y + 25, 4, MUTED)?;
        let clipped: String = value.chars().take(28).collect();
        self.text(conn, &clipped, 85, y + 78, 4, FG)?;
        Ok(())
    }

    fn home_bar<C: Connection>(&self, conn: &C) -> Result<(), Box<dyn Error>> {
        self.fill(
            conn,
            FG,
            Rectangle {
                x: self.width.saturating_div(2).saturating_sub(145) as i16,
                y: self.height.saturating_sub(55) as i16,
                width: 290,
                height: 14,
            },
        )
    }

    fn center_text<C: Connection>(
        &self,
        conn: &C,
        text: &str,
        y: i16,
        scale: u16,
        color: u32,
    ) -> Result<(), Box<dyn Error>> {
        let width = font::text_width(text, scale);
        let x = self.width.saturating_sub(width) / 2;
        self.text(conn, text, x as i16, y, scale, color)
    }

    fn text<C: Connection>(
        &self,
        conn: &C,
        text: &str,
        x: i16,
        y: i16,
        scale: u16,
        color: u32,
    ) -> Result<(), Box<dyn Error>> {
        let rectangles = font::rectangles(text, x, y, scale);
        conn.change_gc(self.gc, &ChangeGCAux::new().foreground(color))?;
        if !rectangles.is_empty() {
            conn.poly_fill_rectangle(self.window, self.gc, &rectangles)?;
        }
        Ok(())
    }

    fn fill<C: Connection>(
        &self,
        conn: &C,
        color: u32,
        rectangle: Rectangle,
    ) -> Result<(), Box<dyn Error>> {
        conn.change_gc(self.gc, &ChangeGCAux::new().foreground(color))?;
        conn.poly_fill_rectangle(self.window, self.gc, &[rectangle])?;
        Ok(())
    }
}
