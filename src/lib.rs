//! Memrium on the phone: the launcher, first version. The home screen is a clock over the wallpaper and a glass dock;
//! a swipe up pulls the sheet of all the apps (a grid in alphabetical order) over it, a swipe down or Back puts it away.
//! A tap starts the app. The list and the icons come from the system through JNI (`apps.rs`).

mod apps;

use apps::{App, Jvm, Msg};
use chebrium::canvas::Canvas;
use chebrium::draw::Align;
use chebrium::mobile::{self, android_activity::AndroidApp, Host, MobileApp, Waker};
use chebrium::picture::Pic;
use chebrium::soft::{Gfx, SoftDraw};
use chebrium::ui::{State, Ui};
use chebrium::widgets;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

/// Logical size of an icon on the grid and in the dock.
const ICON: f32 = 58.0;
const COLS: usize = 4;
const ROW_H: f32 = 96.0;
/// Fling speed (logical px/s) that opens or closes the sheet whatever its position.
const FLING_DECIDES: f32 = 350.0;

/// Apps for the dock, the first installed one of each group.
const DOCK: [&[&str]; 4] = [
    &["com.google.android.dialer", "com.android.contacts", "com.android.dialer"],
    &["com.google.android.apps.messaging", "com.android.mms", "org.telegram.messenger"],
    &["com.android.chrome", "org.mozilla.firefox", "com.mi.globalbrowser", "com.android.browser"],
    &["com.android.camera", "com.google.android.GoogleCamera"],
];

struct Launcher {
    jvm: Jvm,
    /// the Memrium theme's icons packed into the APK: package → PNG (taken by the loader)
    themed: Option<std::collections::HashMap<String, Vec<u8>>>,
    waker: Option<Waker>,
    rx: Option<Receiver<Msg>>,
    apps: Vec<App>,
    dock: Vec<usize>,
    /// the sheet of all apps: 0 put away, 1 over the whole screen
    sheet: f32,
    /// where it is heading when no finger holds it
    target: f32,
    /// the finger is pulling the sheet (not scrolling the grid)
    pulling: bool,
    scroll: f32,
    /// the app pressed this gesture (dimmed), so a press that turned into a pull does not start it
    pressed: Option<usize>,
    /// the wallpaper and the same under the glass, painted once for the screen's size (physical px): the glass of the
    /// dock and of the sheet is a copy of the second one, not a blur of every frame
    backdrop: Option<Backdrop>,
    /// physical px per logical px
    scale: f32,
    show_perf: bool,
    frames: u32,
    fps: f32,
    fps_at: std::time::Instant,
}

/// The local time as (hours, minutes, weekday 0 = Sunday, day, month 0..11, seconds).
fn local_time() -> (i32, i32, i32, i32, i32, i32) {
    // SAFETY: localtime_r only writes the struct it is given
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (tm.tm_hour, tm.tm_min, tm.tm_wday, tm.tm_mday, tm.tm_mon, tm.tm_sec)
    }
}

/// `s` cut to `max` logical px with an ellipsis.
fn fit(ui: &mut Ui<'_>, s: &str, size: f32, max: f32) -> String {
    if ui.measure_raw(s, size, false) <= max {
        return s.to_string();
    }
    let mut out: String = s.to_string();
    while !out.is_empty() {
        out.pop();
        let t = format!("{}…", out.trim_end());
        if ui.measure_raw(&t, size, false) <= max {
            return t;
        }
    }
    "…".into()
}

struct Backdrop {
    size: [usize; 2],
    wall: Arc<Pic>,
    frost: Arc<Pic>,
}

impl Backdrop {
    /// Paint the wallpaper at `size` (physical px), and a copy of it under a glass as big as the screen.
    fn new(size: [usize; 2], scale: f32) -> Backdrop {
        let t0 = std::time::Instant::now();
        let [pw, ph] = size;
        let mut wall = vec![0u8; pw * ph * 4];
        {
            let mut canvas = Canvas::new(&mut wall, pw, ph);
            let mut gfx = Gfx::new();
            let mut painter = SoftDraw::new(&mut canvas, &mut gfx).scaled(scale);
            let mut state = State::default();
            let dt = state.begin_frame();
            let mut ui = Ui::new(&mut painter, &mut state, chebrium::style::Style::new(true, [0.30, 0.62, 1.0]), dt);
            wallpaper(&mut ui, pw as f32 / scale, ph as f32 / scale);
        }
        let mut frost = wall.clone();
        {
            // taller than the screen: no rim at the top and the bottom, the sheet's own edge is drawn over it
            let mut canvas = Canvas::new(&mut frost, pw, ph);
            let m = ph as f32 * 0.25;
            canvas.glass_round_rect(0.0, -m, pw as f32, ph as f32 + 2.0 * m, 0.0, true, 1.0, scale);
        }
        log::info!("memrium: backdrop {pw}x{ph} in {} ms", t0.elapsed().as_millis());
        Backdrop { size, wall: Arc::new(Pic::from_bgra(pw as u32, ph as u32, wall, true, false)), frost: Arc::new(Pic::from_bgra(pw as u32, ph as u32, frost, true, false)) }
    }
}

fn wallpaper(ui: &mut Ui<'_>, w: f32, h: f32) {
    ui.grad([0.0, 0.0, w, h], 0.0, [0.10, 0.16, 0.42], [0.30, 0.10, 0.36], 1.0);
    // soft colour blobs for the glass to bend and blur
    for (cx, cy, r, c, a) in [
        (0.15, 0.18, 0.55, [0.20, 0.75, 0.95], 0.55),
        (0.95, 0.40, 0.60, [0.98, 0.45, 0.55], 0.45),
        (0.30, 0.78, 0.70, [0.55, 0.35, 0.98], 0.50),
        (0.85, 0.95, 0.45, [1.00, 0.75, 0.30], 0.40),
    ] {
        let (x, y, d) = (cx * w, cy * h, r * w);
        ui.fill([x - d / 2.0, y - d / 2.0, d, d], d / 2.0, c, a);
    }
}

/// The part `r` (logical px, rounded by `radius`) of a picture as big as the screen, copied 1:1 row by row: the
/// rounded ends a row at a time, the corners' edge pixels blended by how much of them is inside.
fn copy_rounded(ui: &mut Ui<'_>, pic: &Arc<Pic>, r: [f32; 4], radius: f32, alpha: f32, scale: f32) {
    let (pw, ph) = (pic.pixels().1 as f32, pic.pixels().2 as f32);
    let (x0, x1) = ((r[0] * scale).round().max(0.0), ((r[0] + r[2]) * scale).round().min(pw));
    let (y0, y1) = ((r[1] * scale).round().max(0.0), ((r[1] + r[3]) * scale).round().min(ph));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let rad = (radius * scale).round().min((x1 - x0) / 2.0).min(((r[3] * scale).round()) / 2.0);
    // the rounded rows: the top ones only when the top is on the screen, the bottom ones likewise
    let (top, bottom) = ((r[1] * scale).round(), ((r[1] + r[3]) * scale).round());
    let mut put = |ui: &mut Ui<'_>, x: f32, y: f32, w: f32, hgt: f32, a: f32| {
        if w > 0.0 && hgt > 0.0 {
            ui.picture([x / scale, y / scale, w / scale, hgt / scale], pic, [x, y, w, hgt], a);
        }
    };
    let row = |ui: &mut Ui<'_>, put: &mut dyn FnMut(&mut Ui<'_>, f32, f32, f32, f32, f32), y: f32, d: f32| {
        // d: how far above the corner circles' centre (or below, at the bottom) the middle of this row is
        let inset = rad - (rad * rad - d * d).max(0.0).sqrt();
        let i = inset.floor();
        let cover = 1.0 - (inset - i);
        put(ui, x0 + i + 1.0, y, x1 - x0 - 2.0 * (i + 1.0), 1.0, alpha);
        put(ui, x0 + i, y, 1.0, 1.0, alpha * cover);
        put(ui, x1 - i - 1.0, y, 1.0, 1.0, alpha * cover);
    };
    let (mut b0, mut b1) = (y0, y1);
    if rad > 0.0 {
        let mut y = y0;
        while y < (top + rad).min(y1) {
            row(ui, &mut put, y, top + rad - (y + 0.5));
            y += 1.0;
        }
        b0 = y;
        let mut y = (bottom - rad).max(b0);
        b1 = y;
        while y < y1 {
            row(ui, &mut put, y, y + 0.5 - (bottom - rad));
            y += 1.0;
        }
    }
    put(ui, x0, b0, x1 - x0, b1 - b0, alpha);
}

impl Launcher {
    fn frost(&self, ui: &mut Ui<'_>, r: [f32; 4], radius: f32, alpha: f32) {
        if let Some(b) = &self.backdrop {
            copy_rounded(ui, &b.frost, r, radius, alpha, self.scale);
            // a thin light rim along the top edge
            ui.fill([r[0] + radius, r[1], (r[2] - 2.0 * radius).max(0.0), 1.0], 0.5, [1.0; 3], 0.22 * alpha);
        }
    }

    fn snap(&self, v: f32) -> f32 {
        (v * self.scale).round() / self.scale
    }

    fn spawn_loader(&mut self, scale: f32) {
        let Some(waker) = self.waker.clone() else { return };
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let (jvm, px) = (self.jvm, (ICON * scale).round() as i32);
        let themed = self.themed.take().unwrap_or_default();
        std::thread::spawn(move || apps::load(jvm, px, themed, tx, waker));
    }

    fn pick_dock(&mut self) {
        self.dock = DOCK.iter().filter_map(|group| group.iter().find_map(|p| self.apps.iter().position(|a| a.package == *p))).collect();
        // fill up with the first apps of the list
        let mut i = 0;
        while self.dock.len() < 4 && i < self.apps.len() {
            if !self.dock.contains(&i) {
                self.dock.push(i);
            }
            i += 1;
        }
    }

    /// One app's icon (a letter on a plate until the real one comes). Returns true when it was tapped.
    fn icon(&mut self, ui: &mut Ui<'_>, i: usize, cx: f32, top: f32, size: f32, active: bool) -> bool {
        let size = self.snap(size);
        let r = [self.snap(cx - size / 2.0), self.snap(top), size, size];
        let hot = active && ui.hit(r);
        if hot && ui.click {
            self.pressed = Some(i);
        }
        let down = self.pressed == Some(i) && ui.held;
        let alpha = if down { 0.6 } else { 1.0 };
        let app = &self.apps[i];
        match &app.icon {
            Some(pic) => {
                let (pw, ph) = (pic.pixels().1 as f32, pic.pixels().2 as f32);
                ui.picture(r, pic, [0.0, 0.0, pw, ph], alpha);
            }
            None => {
                ui.fill(r, size * 0.28, [1.0, 1.0, 1.0], 0.22 * alpha);
                let letter: String = app.label.chars().take(1).collect::<String>().to_uppercase();
                ui.text_raw(cx, top + size * 0.64, &letter, size * 0.42, true, [1.0, 1.0, 1.0], 0.9 * alpha, Align::Center);
            }
        }
        // the press ends over the icon it began on: start the app
        let tapped = self.pressed == Some(i) && !ui.held && hot;
        if !ui.held && self.pressed == Some(i) {
            self.pressed = None;
        }
        tapped
    }

    fn launch(&mut self, i: usize) {
        let app = &self.apps[i];
        log::info!("memrium: start {}/{}", app.package, app.activity);
        apps::launch(self.jvm, &app.package, &app.activity);
    }

    /// The finger pulls the sheet up or down; let go, it springs open or shut.
    fn pull(&mut self, ui: &mut Ui<'_>, h: f32, host: &mut Host, grid_at_top: bool) {
        let travel = h * 0.85;
        let wants = ui.scroll != 0.0 && (self.pulling || self.sheet < 0.999 || (grid_at_top && ui.scroll < 0.0));
        if wants && host.finger {
            self.pulling = true;
            self.pressed = None;
            self.sheet = (self.sheet + ui.scroll / travel).clamp(0.0, 1.0);
            ui.scroll = 0.0;
        }
        if self.pulling && !host.finger {
            self.pulling = false;
            self.target = if host.fling > FLING_DECIDES {
                1.0
            } else if host.fling < -FLING_DECIDES {
                0.0
            } else if self.sheet > 0.5 {
                1.0
            } else {
                0.0
            };
            host.stop_fling = true;
            ui.scroll = 0.0;
        }
        if !self.pulling && self.sheet != self.target {
            let k = 1.0 - (-ui.dt() * 13.0).exp();
            self.sheet += (self.target - self.sheet) * k;
            if (self.sheet - self.target).abs() < 0.002 {
                self.sheet = self.target;
            }
            host.redraw = true;
        }
        if self.pulling {
            self.target = self.sheet;
        }
    }
}

impl MobileApp for Launcher {
    fn start(&mut self, waker: Waker) {
        // a frame at the turn of every minute, for the clock
        let w = waker.clone();
        std::thread::spawn(move || loop {
            let sec = local_time().5.clamp(0, 59) as u64;
            std::thread::sleep(std::time::Duration::from_secs(60 - sec));
            w.wake();
        });
        self.waker = Some(waker);
    }

    fn tick(&mut self, _host: &mut Host) {
        let Some(rx) = &self.rx else { return };
        let mut listed = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::List(apps) => {
                    self.apps = apps;
                    listed = true;
                }
                Msg::Icon(i, pic) => {
                    if let Some(a) = self.apps.get_mut(i) {
                        a.icon = Some(pic);
                    }
                }
            }
        }
        if listed {
            self.pick_dock();
        }
    }

    fn draw(&mut self, ui: &mut Ui<'_>, size: [f32; 2], host: &mut Host) {
        if self.rx.is_none() {
            self.spawn_loader(host.scale);
        }
        let [w, h] = size;
        let ins = host.insets;
        let white = [1.0, 1.0, 1.0];
        self.scale = host.scale;
        let px = [(w * host.scale).round() as usize, (h * host.scale).round() as usize];
        if self.backdrop.as_ref().map(|b| b.size) != Some(px) {
            self.backdrop = Some(Backdrop::new(px, host.scale));
        }
        if let Some(b) = &self.backdrop {
            ui.picture([0.0, 0.0, w, h], &b.wall, [0.0, 0.0, px[0] as f32, px[1] as f32], 1.0);
        }

        let grid_at_top = self.scroll <= 0.5;
        self.pull(ui, h, host, grid_at_top);
        let p = self.sheet;
        let home_active = p < 0.01 && !self.pulling;
        let home_alpha = (1.0 - p * 1.6).max(0.0);
        let lift = -p * 40.0;
        let sheet_top = self.snap(ins.top + 8.0 + (h - ins.top) * (1.0 - p));
        // the home screen stays above the sheet's edge (the sheet is a copy, it does not cover what is under it)
        ui.push_clip([0.0, 0.0, w, if p > 0.0 { sheet_top } else { h }]);

        // the clock
        if home_alpha > 0.0 {
            let (hh, mm, wd, day, mon, _) = local_time();
            let days = ["Неділя", "Понеділок", "Вівторок", "Середа", "Четвер", "Пʼятниця", "Субота"];
            let months = ["січня", "лютого", "березня", "квітня", "травня", "червня", "липня", "серпня", "вересня", "жовтня", "листопада", "грудня"];
            let y = ins.top + 36.0 + lift;
            ui.text_raw(w / 2.0, y, &format!("{}, {} {}", days[wd as usize % 7], day, months[mon as usize % 12]), 16.0, true, white, 0.85 * home_alpha, Align::Center);
            ui.text_raw(w / 2.0, y + 82.0, &format!("{hh:02}:{mm:02}"), 84.0, true, white, 0.95 * home_alpha, Align::Center);
            // tap the clock: the frame cost on and off
            if home_active && ui.click && ui.hit([w / 2.0 - 120.0, y + 10.0, 240.0, 80.0]) {
                self.show_perf = !self.show_perf;
            }
        }

        // the dock
        let dock_h = ICON + 24.0;
        let dock = [14.0, h - ins.bottom - dock_h - 10.0 - lift, w - 28.0, dock_h];
        if home_alpha > 0.0 && !self.dock.is_empty() {
            ui.glyph(w / 2.0, dock[1] - 40.0, 18.0, "chevron-up", white, 0.55 * home_alpha);
            ui.text_raw(w / 2.0, dock[1] - 14.0, "проведи вгору: усі програми", 12.0, false, white, 0.55 * home_alpha, Align::Center);
            self.frost(ui, dock, 30.0, home_alpha);
            let step = dock[2] / self.dock.len() as f32;
            for k in 0..self.dock.len() {
                let i = self.dock[k];
                let cx = dock[0] + step * (k as f32 + 0.5);
                if self.icon(ui, i, cx, dock[1] + 12.0, ICON, home_active) {
                    self.launch(i);
                }
            }
        } else if self.apps.is_empty() {
            ui.text_raw(w / 2.0, h / 2.0, "читаю програми…", 15.0, false, white, 0.7, Align::Center);
        }

        ui.pop_clip();

        // the sheet of all apps
        if p > 0.0 {
            let top = sheet_top;
            let sheet = [0.0, top, w, h - top + 40.0];
            self.frost(ui, sheet, 34.0, 1.0);
            ui.fill([w / 2.0 - 20.0, top + 8.0, 40.0, 5.0], 2.5, ui.ink, 0.35);
            ui.text_raw(22.0, top + 44.0, "Програми", 26.0, true, ui.ink, 1.0, Align::Left);
            ui.text_raw(w - 22.0, top + 44.0, &format!("{}", self.apps.len()), 14.0, false, ui.ink, 0.55, Align::Right);
            let area = [8.0, top + 60.0, w - 16.0, h - top - 60.0 - ins.bottom];
            let rows = self.apps.len().div_ceil(COLS);
            let active = p > 0.99 && !self.pulling;
            let s = widgets::scroll_area(ui, area, rows as f32 * ROW_H + 12.0, &mut self.scroll);
            let cell = area[2] / COLS as f32;
            let ink = ui.ink;
            for row in s.rows(ROW_H, rows) {
                for col in 0..COLS {
                    let i = row * COLS + col;
                    if i >= self.apps.len() {
                        break;
                    }
                    let cx = area[0] + cell * (col as f32 + 0.5);
                    let y = s.y(row as f32 * ROW_H) + 8.0;
                    let hit_ok = active && s.hit(ui, [cx - cell / 2.0, y, cell, ROW_H]);
                    if self.icon(ui, i, cx, y, ICON, hit_ok) {
                        self.launch(i);
                    }
                    let label = fit(ui, &self.apps[i].label, 12.0, cell - 14.0);
                    ui.text_raw(cx, y + ICON + 17.0, &label, 12.0, false, ink, 0.9, Align::Center);
                }
            }
            s.end(ui);
        }

        // the cost of the last frame
        self.frames += 1;
        let el = self.fps_at.elapsed().as_secs_f32();
        if el > 0.5 {
            self.fps = self.frames as f32 / el;
            self.frames = 0;
            self.fps_at = std::time::Instant::now();
        }
        if self.show_perf {
            let info = format!("малювання {:.1} мс · копіювання {:.1} мс · {:.0} к/с", host.frame_ms[0], host.frame_ms[1], self.fps);
            let tw = ui.measure_raw(&info, 10.5, false) + 16.0;
            ui.fill([w / 2.0 - tw / 2.0, ins.top + 2.0, tw, 18.0], 9.0, [0.0; 3], 0.55);
            ui.text_raw(w / 2.0, ins.top + 15.0, &info, 10.5, false, white, 0.9, Align::Center);
        }
    }

    fn covers_screen(&self) -> bool {
        true
    }

    fn style(&self, _dark: bool) -> chebrium::style::Style {
        // the wallpaper is dark: dark glass, light ink
        chebrium::style::Style::new(true, [0.30, 0.62, 1.0])
    }

    fn resume(&mut self, _host: &mut Host) {
        // back from an app: the home screen, not the sheet it was started from
        self.sheet = 0.0;
        self.target = 0.0;
        self.pulling = false;
        self.pressed = None;
    }

    fn back(&mut self, _host: &mut Host) -> bool {
        // a launcher never closes; Back puts the sheet away
        self.target = 0.0;
        true
    }
}

/// The icons in the APK's `assets/icons/<package>.png` (see `icons.map`).
fn read_themed(app: &AndroidApp) -> std::collections::HashMap<String, Vec<u8>> {
    let mut out = std::collections::HashMap::new();
    let am = app.asset_manager();
    let Some(dir) = am.open_dir(c"icons") else { return out };
    for name in dir {
        let name = name.to_string_lossy().into_owned();
        let Ok(path) = std::ffi::CString::new(format!("icons/{name}")) else { continue };
        if let Some(mut asset) = am.open(&path) {
            if let Ok(bytes) = asset.buffer() {
                out.insert(name.trim_end_matches(".png").to_string(), bytes.to_vec());
            }
        }
    }
    log::info!("memrium: {} theme icons", out.len());
    out
}

#[no_mangle]
fn android_main(app: AndroidApp) {
    let jvm = Jvm { vm: app.vm_as_ptr() as usize, activity: app.activity_as_ptr() as usize };
    let themed = read_themed(&app);
    let launcher = Launcher {
        jvm,
        themed: Some(themed),
        waker: None,
        rx: None,
        apps: Vec::new(),
        dock: Vec::new(),
        sheet: 0.0,
        target: 0.0,
        pulling: false,
        scroll: 0.0,
        pressed: None,
        backdrop: None,
        scale: 1.0,
        show_perf: true,
        frames: 0,
        fps: 0.0,
        fps_at: std::time::Instant::now(),
    };
    mobile::run(app, launcher);
}
