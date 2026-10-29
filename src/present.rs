//! Vendored from Moon (OSS-Phone-V1 `src/a26-shell/src`); keep in sync.
#![allow(dead_code)]
//! Uploads a `Canvas` to an X11 window by sending only changed pixels.
//!
//! The presenter keeps a server-side pixmap that always mirrors the last
//! presented frame and a client-side copy of it. Each frame is compared in
//! horizontal bands; only the changed span of each band is sent with
//! `PutImage` into the pixmap, and then the union of the damage is copied to
//! the window in a single `CopyArea`, so the panel never scans out a
//! half-drawn frame. Every request is one-way: no reply is ever awaited, as
//! Moon's long-lived connection requires (see `X11-INPUT-DEADLOCK.md`).

use std::error::Error;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use x11rb::connection::Connection;
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    ConnectionExt as _, CreateGCAux, Gcontext, ImageFormat, Pixmap, Window,
};

use crate::gfx::Canvas;

/// Keep each PutImage comfortably under the core 256 KiB request limit so
/// x11rb never needs BIG-REQUESTS (whose negotiation is a reply wait).
const MAX_REQUEST_BYTES: usize = 200 * 1024;
const BAND_ROWS: i32 = 16;
/// A frame changing at least this much (a fifth of the screen) goes
/// through the shared buffer (`Shared`); a smaller one (a scroll strip, a
/// streamed line) through the socket, which never waits for the server.
const SHM_FROM_BYTES: usize = 2 * 1024 * 1024;
const CHUNK: usize = 64;

/// Whether two runs of pixels are the same, 16 at a time (vectorised): a
/// slice's `==` is musl's `memcmp`, which compares one byte at a time, and
/// a row the frame drew again unchanged (most of a page's background) cost
/// 13,000 instructions.
fn same(a: &[u32], b: &[u32]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let (mut x, mut y) = (a.chunks_exact(16), b.chunks_exact(16));
    for (x, y) in (&mut x).zip(&mut y) {
        let mut diff = 0;
        for i in 0..16 {
            diff |= x[i] ^ y[i];
        }
        if diff != 0 {
            return false;
        }
    }
    x.remainder().iter().zip(y.remainder()).all(|(p, q)| p == q)
}

pub struct Presenter {
    window: Window,
    pixmap: Pixmap,
    gc: Gcontext,
    width: i32,
    height: i32,
    shadow: Vec<u32>,
    valid: bool,
    scratch: Vec<u32>,
    /// Pixmap areas changed server-side (`scroll`) that the window still
    /// has to get with the next present.
    moved: Vec<Damage>,
    /// Rows the server holds that `shadow` does not (moved there by
    /// `scroll`): uploaded, not compared, when next presented.
    stale: Vec<bool>,
    /// The MIT-SHM upload path, when enabled (`enable_shm`).
    shared: Option<Shared>,
    /// Areas of this frame to send from `shared` (`x0, y0, x1, y1`).
    puts: Vec<(i32, i32, i32, i32)>,
}

/// A buffer the X server maps (MIT-SHM 1.2, a memfd passed over the
/// socket): an upload is then a copy into it and one in the server, not the
/// pixels through the X socket (a full 1080x2340 frame is 10 MB, which took
/// 5-15 ms on the A26). Laid out like the canvas. An added fast path, not in
/// Moon's copy.
struct Shared {
    seg: shm::Seg,
    map: *mut u32,
    len: usize,
    /// The sequence number of the last frame's last ShmPutImage, until its
    /// completion event says the server has read the buffer: it is not
    /// written to before then.
    in_flight: Option<u16>,
}

impl Shared {
    fn create<C: Connection>(conn: &C, width: i32, height: i32) -> Result<Self, Box<dyn Error>> {
        let version = conn.shm_query_version()?.reply()?;
        if (version.major_version, version.minor_version) < (1, 2) {
            return Err("MIT-SHM without file descriptors".into());
        }
        let len = (width * height) as usize;
        let bytes = len * 4;
        // SAFETY: a name and flags; returns a new fd or -1.
        let fd = unsafe { libc::memfd_create(c"a26-present".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: a freshly created descriptor this value now owns.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: sizes our own memfd.
        if unsafe { libc::ftruncate(fd.as_raw_fd(), bytes as libc::off_t) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: maps `bytes` of our memfd, shared; checked for failure.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error().into());
        }
        let seg = conn.generate_id()?;
        // Checked: a server that cannot map it says so now, rather than as a
        // window that never updates.
        let attached = conn
            .shm_attach_fd(seg, fd, true)
            .map_err(Box::<dyn Error>::from)
            .and_then(|cookie| cookie.check().map_err(Box::<dyn Error>::from));
        if let Err(error) = attached {
            // SAFETY: unmaps exactly what was mapped above.
            unsafe { libc::munmap(map, bytes) };
            return Err(error);
        }
        Ok(Self {
            seg,
            map: map.cast(),
            len,
            in_flight: None,
        })
    }

    fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: `map` is a live mapping of `len` pixels owned by this value
        // (unmapped only in `destroy`), and the server only reads it.
        unsafe { std::slice::from_raw_parts_mut(self.map, self.len) }
    }

    fn destroy<C: Connection>(&self, conn: &C) {
        let _ = conn.shm_detach(self.seg);
        // SAFETY: unmaps the mapping made in `create`, used no more.
        unsafe { libc::munmap(self.map.cast(), self.len * 4) };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Damage {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Presenter {
    pub fn create<C: Connection>(
        conn: &C,
        window: Window,
        depth: u8,
        width: u16,
        height: u16,
    ) -> Result<Self, Box<dyn Error>> {
        let pixmap = conn.generate_id()?;
        conn.create_pixmap(depth, pixmap, window, width, height)?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, pixmap, &CreateGCAux::new().graphics_exposures(0))?;
        let width = i32::from(width);
        let height = i32::from(height);
        Ok(Self {
            window,
            pixmap,
            gc,
            width,
            height,
            shadow: vec![0; (width * height) as usize],
            valid: false,
            scratch: Vec::new(),
            moved: Vec::new(),
            stale: vec![false; height as usize],
            shared: None,
            puts: Vec::new(),
        })
    }

    /// Uploads through MIT-SHM from now on, if the server can; returns
    /// whether it does. The caller then passes X events to `note_event`.
    pub fn enable_shm<C: Connection>(&mut self, conn: &C) -> bool {
        if self.shared.is_none() {
            self.shared = Shared::create(conn, self.width, self.height).ok();
        }
        self.shared.is_some()
    }

    /// Sees an X event: a ShmPutImage's completion frees the buffer.
    pub fn note_event(&mut self, event: &x11rb::protocol::Event) {
        if let (Some(shared), x11rb::protocol::Event::ShmCompletion(done)) =
            (&mut self.shared, event)
            && done.shmseg == shared.seg
            && shared.in_flight == Some(done.sequence)
        {
            shared.in_flight = None;
        }
    }

    /// Forgets what the server holds, forcing the next frame to upload fully.
    pub fn invalidate(&mut self) {
        self.valid = false;
        self.moved.clear();
    }

    /// Moves rows `y0..y1` of the presented frame by `dy` inside the server's
    /// pixmap (an added scrolling fast path for long lists, not in Moon's
    /// copy): the next `present` then uploads only what the move did not
    /// bring. One-way, like everything here.
    pub fn scroll<C: Connection>(
        &mut self,
        conn: &C,
        y0: i32,
        y1: i32,
        dy: i32,
    ) -> Result<(), Box<dyn Error>> {
        let (y0, y1) = (y0.max(0), y1.min(self.height));
        let rows = y1 - y0 - dy.abs();
        if !self.valid || dy == 0 || rows <= 0 {
            return Ok(());
        }
        let (src, dst) = if dy > 0 { (y0, y0 + dy) } else { (y0 - dy, y0) };
        conn.copy_area(
            self.pixmap,
            self.pixmap,
            self.gc,
            0,
            src as i16,
            0,
            dst as i16,
            self.width as u16,
            rows as u16,
        )?;
        // Rather than moving the shadow too (megabytes per frame), the
        // moved rows are no longer known to match it.
        self.stale[dst as usize..(dst + rows) as usize].fill(true);
        self.moved.push(Damage {
            x0: 0,
            y0: dst,
            x1: self.width,
            y1: dst + rows,
        });
        Ok(())
    }

    /// Uploads `canvas` and copies the damaged areas to the window, whose
    /// origin corresponds to canvas row `window_y`. Returns the bounding box
    /// of the damage.
    ///
    /// Each 16-row band is compared in 64-pixel column chunks; runs of dirty
    /// chunks are uploaded separately, so a moving edge on either side of a
    /// large unchanged area costs only the edges.
    pub fn present<C: Connection>(
        &mut self,
        conn: &C,
        canvas: &Canvas,
        window_y: i32,
    ) -> Result<Option<Damage>, Box<dyn Error>> {
        let all = [(0, self.height)];
        self.present_rows(conn, canvas, window_y, &all)
    }

    /// `present`, comparing only rows `rows` (each `y0..y1`): the caller
    /// knows the canvas changed nowhere else since the last present (beyond
    /// what `scroll` moved). An added fast path, not in Moon's copy.
    pub fn present_rows<C: Connection>(
        &mut self,
        conn: &C,
        canvas: &Canvas,
        window_y: i32,
        rows: &[(i32, i32)],
    ) -> Result<Option<Damage>, Box<dyn Error>> {
        self.present_rows_and_areas(conn, canvas, window_y, rows, &[])
    }

    /// `present_rows`, and areas (`x0, y0, x1, y1`) uploaded as the canvas
    /// has them, whether their rows are stale or not, leaving those rows'
    /// state as it was: the caller drew only these columns anew in rows
    /// that otherwise hold what the server has (moved there by `scroll`), so
    /// a floating thing redrawn over moved rows costs its own columns, not
    /// whole rows. One upload for both (a second would wait a round trip
    /// for the first). An added fast path, not in Moon's copy.
    pub fn present_rows_and_areas<C: Connection>(
        &mut self,
        conn: &C,
        canvas: &Canvas,
        window_y: i32,
        rows: &[(i32, i32)],
        areas: &[(i32, i32, i32, i32)],
    ) -> Result<Option<Damage>, Box<dyn Error>> {
        if canvas.width != self.width || canvas.height != self.height {
            return Err("canvas size does not match presenter".into());
        }
        let all = [(0, self.height)];
        let rows = if self.valid { rows } else { &all[..] };
        let width = self.width as usize;
        let chunks = width.div_ceil(CHUNK);
        let mut dirty = vec![false; chunks];
        let mut runs: Vec<(i32, i32)> = Vec::new();
        // Damage rectangles still growing downward, merged across bands.
        let mut open: Vec<Damage> = Vec::new();
        let mut closed: Vec<Damage> = Vec::new();
        for &(y0, y1) in rows {
            let (y0, y1) = (y0.clamp(0, self.height), y1.clamp(0, self.height));
            let mut band_y = y0;
            while band_y < y1 {
                let band_end = (band_y + BAND_ROWS).min(y1);
                let stale = self.stale[band_y as usize..band_end as usize]
                    .iter()
                    .any(|row| *row);
                dirty.fill(!self.valid || stale);
                if self.valid && !stale {
                    for y in band_y..band_end {
                        let start = y as usize * width;
                        let new = &canvas.pixels[start..start + width];
                        let old = &self.shadow[start..start + width];
                        if same(new, old) {
                            continue;
                        }
                        for (chunk, flag) in dirty.iter_mut().enumerate() {
                            if *flag {
                                continue;
                            }
                            let a = chunk * CHUNK;
                            let b = (a + CHUNK).min(width);
                            *flag = !same(&new[a..b], &old[a..b]);
                        }
                    }
                }
                runs.clear();
                let mut chunk = 0;
                while chunk < chunks {
                    if !dirty[chunk] {
                        chunk += 1;
                        continue;
                    }
                    let first = chunk;
                    // Bridge single clean chunks to keep the request count low.
                    while chunk < chunks
                        && (dirty[chunk] || (chunk + 1 < chunks && dirty[chunk + 1]))
                    {
                        chunk += 1;
                    }
                    let x0 = (first * CHUNK) as i32;
                    let x1 = ((chunk * CHUNK).min(width)) as i32;
                    runs.push((x0, x1));
                }
                self.stale[band_y as usize..band_end as usize].fill(false);
                let mut still_open = Vec::with_capacity(runs.len());
                for &(x0, x1) in &runs {
                    self.puts.push((x0, band_y, x1, band_end));
                    if let Some(position) = open
                        .iter()
                        .position(|d| d.x0 == x0 && d.x1 == x1 && d.y1 == band_y)
                    {
                        let mut grown = open.swap_remove(position);
                        grown.y1 = band_end;
                        still_open.push(grown);
                    } else {
                        still_open.push(Damage {
                            x0,
                            y0: band_y,
                            x1,
                            y1: band_end,
                        });
                    }
                }
                closed.append(&mut open);
                open = still_open;
                band_y = band_end;
            }
        }
        closed.append(&mut open);
        // Not valid, everything was compared and uploaded.
        if self.valid {
            for &(x0, y0, x1, y1) in areas {
                let (x0, x1) = (x0.clamp(0, self.width), x1.clamp(0, self.width));
                let (y0, y1) = (y0.clamp(0, self.height), y1.clamp(0, self.height));
                if x1 > x0 && y1 > y0 {
                    self.puts.push((x0, y0, x1, y1));
                    closed.push(Damage { x0, y0, x1, y1 });
                }
            }
        }
        closed.append(&mut self.moved);
        self.upload(conn, canvas)?;
        self.valid = true;
        let mut bounds: Option<Damage> = None;
        for damage in &closed {
            self.copy_to_window(conn, *damage, window_y)?;
            bounds = Some(match bounds {
                Some(b) => Damage {
                    x0: b.x0.min(damage.x0),
                    y0: b.y0.min(damage.y0),
                    x1: b.x1.max(damage.x1),
                    y1: b.y1.max(damage.y1),
                },
                None => *damage,
            });
        }
        Ok(bounds)
    }

    /// Repaints the window from the pixmap after an Expose, without upload.
    pub fn expose<C: Connection>(&self, conn: &C, window_y: i32) -> Result<(), Box<dyn Error>> {
        if self.valid {
            self.copy_to_window(
                conn,
                Damage {
                    x0: 0,
                    y0: 0,
                    x1: self.width,
                    y1: self.height,
                },
                window_y,
            )?;
        }
        Ok(())
    }

    fn copy_to_window<C: Connection>(
        &self,
        conn: &C,
        damage: Damage,
        window_y: i32,
    ) -> Result<(), Box<dyn Error>> {
        let y0 = damage.y0.max(window_y);
        if damage.y1 <= y0 {
            return Ok(());
        }
        conn.copy_area(
            self.pixmap,
            self.window,
            self.gc,
            damage.x0 as i16,
            y0 as i16,
            damage.x0 as i16,
            (y0 - window_y) as i16,
            (damage.x1 - damage.x0) as u16,
            (damage.y1 - y0) as u16,
        )?;
        Ok(())
    }

    /// Sends this frame's changed areas (`puts`) and keeps them in the
    /// shadow: a large frame through the shared buffer, if there is one
    /// (the last area asking for a completion event: the buffer is not
    /// written to again before it), the rest through the socket, which
    /// never waits.
    fn upload<C: Connection>(&mut self, conn: &C, canvas: &Canvas) -> Result<(), Box<dyn Error>> {
        let bytes: usize = self
            .puts
            .iter()
            .map(|&(x0, y0, x1, y1)| ((x1 - x0) * (y1 - y0) * 4) as usize)
            .sum();
        if let Some(shared) = &mut self.shared
            && bytes >= SHM_FROM_BYTES
        {
            if shared.in_flight.is_some() {
                // The server may still be reading the last one: a round
                // trip makes sure it has.
                conn.get_input_focus()?.reply()?;
                shared.in_flight = None;
            }
            let pixels = shared.pixels();
            for &(x0, y0, x1, y1) in &self.puts {
                let span = (x1 - x0) as usize;
                for row in y0..y1 {
                    let start = (row * self.width + x0) as usize;
                    let from = &canvas.pixels[start..start + span];
                    // A stale row's copy is not compared (it is sent whole).
                    if !self.stale[row as usize] {
                        self.shadow[start..start + span].copy_from_slice(from);
                    }
                    pixels[start..start + span].copy_from_slice(from);
                }
            }
            let last = self.puts.len().saturating_sub(1);
            for (index, &(x0, y0, x1, y1)) in self.puts.iter().enumerate() {
                let cookie = conn.shm_put_image(
                    self.pixmap,
                    self.gc,
                    self.width as u16,
                    self.height as u16,
                    x0 as u16,
                    y0 as u16,
                    (x1 - x0) as u16,
                    (y1 - y0) as u16,
                    x0 as i16,
                    y0 as i16,
                    24,
                    ImageFormat::Z_PIXMAP.into(),
                    index == last,
                    shared.seg,
                    0,
                )?;
                if index == last {
                    shared.in_flight = Some(cookie.sequence_number() as u16);
                }
            }
            self.puts.clear();
            return Ok(());
        }
        for (x0, y0, x1, y1) in std::mem::take(&mut self.puts) {
            self.put(conn, canvas, x0, y0, x1, y1)?;
        }
        Ok(())
    }

    /// Sends one area through the socket, in requests under the core limit.
    fn put<C: Connection>(
        &mut self,
        conn: &C,
        canvas: &Canvas,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
    ) -> Result<(), Box<dyn Error>> {
        let span = (x1 - x0) as usize;
        let rows_per_request = (MAX_REQUEST_BYTES / (span * 4)).max(1) as i32;
        let mut y = y0;
        while y < y1 {
            let rows = rows_per_request.min(y1 - y);
            self.scratch.clear();
            for row in y..y + rows {
                let start = (row * self.width + x0) as usize;
                let pixels = &canvas.pixels[start..start + span];
                // A stale row's copy is not compared (it is sent whole).
                if !self.stale[row as usize] {
                    self.shadow[start..start + span].copy_from_slice(pixels);
                }
                self.scratch.extend_from_slice(pixels);
            }
            conn.put_image(
                ImageFormat::Z_PIXMAP,
                self.pixmap,
                self.gc,
                span as u16,
                rows as u16,
                x0 as i16,
                y as i16,
                0,
                24,
                pixel_bytes(&self.scratch),
            )?;
            y += rows;
        }
        Ok(())
    }

    pub fn destroy<C: Connection>(&self, conn: &C) {
        if let Some(shared) = &self.shared {
            shared.destroy(conn);
        }
        let _ = conn.free_pixmap(self.pixmap);
        let _ = conn.free_gc(self.gc);
    }
}

/// Views XRGB pixels as the little-endian Z-pixmap bytes Xorg expects.
#[cfg(target_endian = "little")]
fn pixel_bytes(pixels: &[u32]) -> &[u8] {
    // SAFETY: u32 has no padding or invalid bit patterns, u8 has alignment 1,
    // and the byte length covers exactly the same allocation.
    unsafe { std::slice::from_raw_parts(pixels.as_ptr().cast::<u8>(), pixels.len() * 4) }
}

#[cfg(test)]
mod tests {
    use super::same;

    #[test]
    fn rows_compare_like_slices() {
        let a: Vec<u32> = (0..1080).collect();
        let mut b = a.clone();
        assert!(same(&a, &b));
        for at in [0, 15, 16, 1070, 1079] {
            b[at] ^= 1;
            assert!(!same(&a, &b), "{at}");
            b[at] ^= 1;
        }
        assert!(!same(&a, &b[..1079]));
        assert!(same(&a[3..40], &b[3..40]));
    }
}
