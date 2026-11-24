//! The phone's programs, through JNI (the APK has no Java code of its own): the list of launchable activities, their
//! icons drawn into bitmaps, and starting one.
//!
//! The list and the icons are read on a thread of their own: the list first (the screen can lay out names at once), then
//! the icons one by one, each sent as it is ready.

use chebrium::mobile::Waker;
use chebrium::picture::Pic;
use jni::objects::{JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// The activity's Java VM and its object, as plain numbers so they can go to another thread. Both live as long as
/// the process.
#[derive(Clone, Copy)]
pub struct Jvm {
    pub vm: usize,
    pub activity: usize,
}

pub struct App {
    pub label: String,
    pub package: String,
    pub activity: String,
    pub icon: Option<Arc<Pic>>,
}

pub enum Msg {
    List(Vec<App>),
    /// the icon of the app at this index of the list
    Icon(usize, Arc<Pic>),
}

impl Jvm {
    /// Run `f` with a JNI environment on this thread and the activity. A Java exception is logged, cleared and
    /// becomes `None`.
    fn with<R>(&self, f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<R>) -> Option<R> {
        // SAFETY: the pointers are the process's VM and the activity's global reference (see `Jvm`)
        let vm = unsafe { JavaVM::from_raw(self.vm as *mut jni::sys::JavaVM) }.ok()?;
        let mut env = vm.attach_current_thread_permanently().ok()?;
        let activity = unsafe { JObject::from_raw(self.activity as jni::sys::jobject) };
        let r = f(&mut env, &activity);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        match r {
            Ok(r) => Some(r),
            Err(e) => {
                log::warn!("memrium: jni: {e}");
                None
            }
        }
    }
}

fn string(env: &mut JNIEnv, obj: JObject) -> jni::errors::Result<String> {
    if obj.is_null() {
        return Ok(String::new());
    }
    let s = JString::from(obj);
    let out: String = env.get_string(&s)?.into();
    env.delete_local_ref(s)?;
    Ok(out)
}

/// `new Intent(ACTION_MAIN).addCategory(CATEGORY_LAUNCHER)`
fn launcher_intent<'a>(env: &mut JNIEnv<'a>) -> jni::errors::Result<JObject<'a>> {
    let action = env.new_string("android.intent.action.MAIN")?;
    let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[JValue::Object(&action)])?;
    let cat = env.new_string("android.intent.category.LAUNCHER")?;
    env.call_method(&intent, "addCategory", "(Ljava/lang/String;)Landroid/content/Intent;", &[JValue::Object(&cat)])?;
    Ok(intent)
}

/// Read the list, send it, then draw and send each icon at `px` × `px`. An app in `themed` (package → PNG) gets that
/// picture instead of its own icon.
pub fn load(jvm: Jvm, px: i32, themed: HashMap<String, Vec<u8>>, tx: Sender<Msg>, waker: Waker) {
    let own = "ua.memrium.mobile";
    jvm.with(|env, activity| {
        let pm = env.call_method(activity, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[])?.l()?;
        let intent = launcher_intent(env)?;
        let list = env.call_method(&pm, "queryIntentActivities", "(Landroid/content/Intent;I)Ljava/util/List;", &[JValue::Object(&intent), JValue::Int(0)])?.l()?;
        let n = env.call_method(&list, "size", "()I", &[])?.i()?;
        // (label, package, activity, index in the Java list)
        let mut found = Vec::new();
        for i in 0..n {
            let row = env.with_local_frame(16, |env| -> jni::errors::Result<(String, String, String)> {
                let ri = env.call_method(&list, "get", "(I)Ljava/lang/Object;", &[JValue::Int(i)])?.l()?;
                let label = env.call_method(&ri, "loadLabel", "(Landroid/content/pm/PackageManager;)Ljava/lang/CharSequence;", &[JValue::Object(&pm)])?.l()?;
                let label = env.call_method(&label, "toString", "()Ljava/lang/String;", &[])?.l()?;
                let label = string(env, label)?;
                let ai = env.get_field(&ri, "activityInfo", "Landroid/content/pm/ActivityInfo;")?.l()?;
                let pkg = env.get_field(&ai, "packageName", "Ljava/lang/String;")?.l()?;
                let pkg = string(env, pkg)?;
                let name = env.get_field(&ai, "name", "Ljava/lang/String;")?.l()?;
                let name = string(env, name)?;
                Ok((label, pkg, name))
            });
            match row {
                Ok((label, pkg, name)) if pkg != own => found.push((label, pkg, name, i)),
                Ok(_) => {}
                Err(e) => {
                    log::warn!("memrium: app {i}: {e}");
                    let _ = env.exception_clear();
                }
            }
        }
        found.sort_by_key(|a| a.0.to_lowercase());
        log::info!("memrium: {} apps", found.len());
        let apps = found.iter().map(|(l, p, a, _)| App { label: l.clone(), package: p.clone(), activity: a.clone(), icon: None }).collect();
        let _ = tx.send(Msg::List(apps));
        waker.wake();

        let t0 = std::time::Instant::now();
        let cfg = env.get_static_field("android/graphics/Bitmap$Config", "ARGB_8888", "Landroid/graphics/Bitmap$Config;")?.l()?;
        let mut buf = vec![0u8; (px * px * 4) as usize];
        for (k, (_, pkg, _, i)) in found.iter().enumerate() {
            if let Some(pic) = themed.get(pkg).and_then(|png| themed_icon(png, px as usize)) {
                let _ = tx.send(Msg::Icon(k, Arc::new(pic)));
                continue;
            }
            let drawn = env.with_local_frame(16, |env| -> jni::errors::Result<()> {
                let ri = env.call_method(&list, "get", "(I)Ljava/lang/Object;", &[JValue::Int(*i)])?.l()?;
                let icon = env.call_method(&ri, "loadIcon", "(Landroid/content/pm/PackageManager;)Landroid/graphics/drawable/Drawable;", &[JValue::Object(&pm)])?.l()?;
                let bmp = env
                    .call_static_method("android/graphics/Bitmap", "createBitmap", "(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;", &[JValue::Int(px), JValue::Int(px), JValue::Object(&cfg)])?
                    .l()?;
                let canvas = env.new_object("android/graphics/Canvas", "(Landroid/graphics/Bitmap;)V", &[JValue::Object(&bmp)])?;
                env.call_method(&icon, "setBounds", "(IIII)V", &[JValue::Int(0), JValue::Int(0), JValue::Int(px), JValue::Int(px)])?;
                env.call_method(&icon, "draw", "(Landroid/graphics/Canvas;)V", &[JValue::Object(&canvas)])?;
                // SAFETY: `buf` outlives the buffer object, which is only used inside this frame
                let bb = unsafe { env.new_direct_byte_buffer(buf.as_mut_ptr(), buf.len()) }?;
                env.call_method(&bmp, "copyPixelsToBuffer", "(Ljava/nio/Buffer;)V", &[JValue::Object(&bb)])?;
                env.call_method(&bmp, "recycle", "()V", &[])?;
                Ok(())
            });
            match drawn {
                Ok(()) => {
                    // ARGB_8888 is premultiplied R G B A in memory; a Pic wants premultiplied B G R A
                    let mut bgra = buf.clone();
                    glass_edges(&mut bgra, px as usize);
                    for p in bgra.chunks_exact_mut(4) {
                        p.swap(0, 2);
                    }
                    let pic = Arc::new(Pic::from_bgra(px as u32, px as u32, bgra, false, true));
                    let _ = tx.send(Msg::Icon(k, pic));
                    if k % 6 == 5 {
                        waker.wake();
                    }
                }
                Err(e) => {
                    log::warn!("memrium: icon {k}: {e}");
                    let _ = env.exception_clear();
                }
            }
        }
        waker.wake();
        log::info!("memrium: {} icons in {} ms", found.len(), t0.elapsed().as_millis());
        Ok(())
    });
}

/// Start an app's activity in its own task (or bring that task to the front).
pub fn launch(jvm: Jvm, package: &str, activity: &str) -> bool {
    jvm.with(|env, act| {
        env.with_local_frame(16, |env| {
            let intent = launcher_intent(env)?;
            let (p, a) = (env.new_string(package)?, env.new_string(activity)?);
            env.call_method(&intent, "setClassName", "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;", &[JValue::Object(&p), JValue::Object(&a)])?;
            // FLAG_ACTIVITY_NEW_TASK | FLAG_ACTIVITY_RESET_TASK_IF_NEEDED
            env.call_method(&intent, "setFlags", "(I)Landroid/content/Intent;", &[JValue::Int(0x1020_0000)])?;
            env.call_method(act, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
            Ok(())
        })
    })
    .is_some()
}

/// A box blur of `src` (w × h) with radius `r`, three passes each way (close to a gaussian). Outside the picture is 0
/// (transparent): an icon whose shape fills its square still has edges along the square's sides.
fn blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut a = src.to_vec();
    let mut b = vec![0f32; a.len()];
    let n = (2 * r + 1) as f32;
    for _ in 0..3 {
        for y in 0..h {
            let row = &a[y * w..(y + 1) * w];
            let at = |x: isize| if x < 0 || x >= w as isize { 0.0 } else { row[x as usize] };
            let mut acc: f32 = (-(r as isize)..=r as isize).map(at).sum();
            for x in 0..w {
                b[y * w + x] = acc / n;
                acc += at(x as isize + r as isize + 1) - at(x as isize - r as isize);
            }
        }
        for x in 0..w {
            let at = |y: isize| if y < 0 || y >= h as isize { 0.0 } else { b[y as usize * w + x] };
            let mut acc: f32 = (-(r as isize)..=r as isize).map(at).sum();
            for y in 0..h {
                a[y * w + x] = acc / n;
                acc += at(y as isize + r as isize + 1) - at(y as isize - r as isize);
            }
        }
    }
    a
}

/// Glass edges on an icon (premultiplied, 4 bytes a pixel, `px` × `px`, any channel order): a thin bright rim along
/// the outline of its shape, strongest where the edge faces the light (top left) with a weaker one on the opposite
/// side, and a faint bevel inside it (lighter towards the light, darker away from it), as Liquid Glass icons.
pub fn glass_edges(buf: &mut [u8], px: usize) {
    let alpha: Vec<f32> = buf.chunks_exact(4).map(|p| p[3] as f32 / 255.0).collect();
    let rim_r = ((px as f32 * 0.02).round() as usize).max(1);
    let bevel_r = ((px as f32 * 0.045).round() as usize).max(2);
    let rim = blur(&alpha, px, px, rim_r);
    let bevel = blur(&alpha, px, px, bevel_r);
    // the light comes from above, a little from the left
    let (lx, ly) = (-0.45f32, -0.89f32);
    let at = |v: &[f32], x: usize, y: usize| v[y.min(px - 1) * px + x.min(px - 1)];
    for y in 0..px {
        for x in 0..px {
            let a = alpha[y * px + x];
            if a <= 0.0 {
                continue;
            }
            let (xm, ym) = (x.saturating_sub(1), y.saturating_sub(1));
            // the outward normal: down the slope of the blurred shape
            let normal = |v: &[f32]| {
                let gx = at(v, x + 1, y) - at(v, xm, y);
                let gy = at(v, x, y + 1) - at(v, x, ym);
                let l = (gx * gx + gy * gy).sqrt();
                if l < 1e-5 {
                    (0.0, 0.0)
                } else {
                    (-gx / l, -gy / l)
                }
            };
            // the rim: pixels close to the outline (the blurred alpha drops there)
            let edge = ((0.97 - rim[y * px + x]) / 0.42).clamp(0.0, 1.0).powf(1.3);
            let (nx, ny) = normal(&rim);
            let facing = nx * lx + ny * ly;
            let light = 0.2 + 0.8 * facing.max(0.0).powf(1.2) + 0.6 * (-facing).max(0.0).powf(1.5);
            let t_rim = (edge * light * 0.8).min(0.85);
            // the bevel: a wider band, lit on the side towards the light and shaded on the other
            let band = (1.0 - bevel[y * px + x]).clamp(0.0, 1.0) * 1.6;
            let (bx, by) = normal(&bevel);
            let shade = (bx * lx + by * ly) * band.min(1.0);
            let p = &mut buf[(y * px + x) * 4..][..4];
            for c in 0..3 {
                let mut v = p[c] as f32;
                // bevel: towards white on the lit side, towards black on the other
                if shade > 0.0 {
                    v += (a * 255.0 - v) * shade * 0.22;
                } else {
                    v *= 1.0 + shade * 0.18;
                }
                // rim: towards white
                v += (a * 255.0 - v) * t_rim;
                p[c] = v.round().clamp(0.0, a * 255.0) as u8;
            }
        }
    }
}

/// A PNG of the icon theme as a picture of `px` × `px`: cut to the visible shape (the theme leaves a margin round
/// it, the system's icons have none), fitted, scaled down by area.
fn themed_icon(png_bytes: &[u8], px: usize) -> Option<Pic> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut raw = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut raw).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    // premultiplied RGBA in 0..1
    let src: Vec<[f32; 4]> = match info.color_type {
        png::ColorType::Rgba => raw.chunks_exact(4).take(w * h).map(|p| [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]).collect::<Vec<_>>(),
        png::ColorType::Rgb => raw.chunks_exact(3).take(w * h).map(|p| [p[0] as f32, p[1] as f32, p[2] as f32, 255.0]).collect::<Vec<_>>(),
        png::ColorType::GrayscaleAlpha => raw.chunks_exact(2).take(w * h).map(|p| [p[0] as f32, p[0] as f32, p[0] as f32, p[1] as f32]).collect::<Vec<_>>(),
        png::ColorType::Grayscale => raw.iter().take(w * h).map(|&g| [g as f32, g as f32, g as f32, 255.0]).collect::<Vec<_>>(),
        _ => return None,
    }
    .into_iter()
    .map(|[r, g, b, a]| {
        let a = a / 255.0;
        [r / 255.0 * a, g / 255.0 * a, b / 255.0 * a, a]
    })
    .collect();
    // the visible part
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if src[y * w + x][3] > 0.04 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    // a square round it, centred
    let side = (x1 - x0).max(y1 - y0) as f32;
    let (ox, oy) = ((x0 + x1) as f32 / 2.0 - side / 2.0, (y0 + y1) as f32 / 2.0 - side / 2.0);
    let step = side / px as f32;
    let sample = |x: isize, y: isize| -> [f32; 4] {
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
            [0.0; 4]
        } else {
            src[y as usize * w + x as usize]
        }
    };
    let mut out = vec![0u8; px * px * 4];
    for dy in 0..px {
        let (sy0, sy1) = (oy + dy as f32 * step, oy + (dy + 1) as f32 * step);
        for dx in 0..px {
            let (sx0, sx1) = (ox + dx as f32 * step, ox + (dx + 1) as f32 * step);
            let mut acc = [0f32; 4];
            let mut sy = sy0.floor();
            while sy < sy1 {
                let wy = (sy + 1.0).min(sy1) - sy.max(sy0);
                let mut sx = sx0.floor();
                while sx < sx1 {
                    let wx = (sx + 1.0).min(sx1) - sx.max(sx0);
                    let c = sample(sx as isize, sy as isize);
                    for k in 0..4 {
                        acc[k] += c[k] * wx * wy;
                    }
                    sx += 1.0;
                }
                sy += 1.0;
            }
            let area = step * step;
            // B G R A for the picture
            let o = (dy * px + dx) * 4;
            out[o] = (acc[2] / area * 255.0).round().clamp(0.0, 255.0) as u8;
            out[o + 1] = (acc[1] / area * 255.0).round().clamp(0.0, 255.0) as u8;
            out[o + 2] = (acc[0] / area * 255.0).round().clamp(0.0, 255.0) as u8;
            out[o + 3] = (acc[3] / area * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    Some(Pic::from_bgra(px as u32, px as u32, out, false, true))
}
