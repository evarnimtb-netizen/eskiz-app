//! The Android side of the file bridge (A4): the Storage Access Framework file picker, the share
//! sheet, the clipboard for images and Back (design sections 3.1 to 3.8).
//!
//! Rust calls instance methods of `EskizActivity` through the `jni` crate (0.22), each of which
//! only queues work and returns at once. The Java side answers through the `extern "system"`
//! functions at the end of this file, which resolve the request in [`crate::saf::global`] and wake
//! the frame loop. Everything that can be tested without a phone lives in `saf.rs`; this file is
//! the thin, Android-only part.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use jni::errors::Result as JniResult;
use jni::objects::{JClass, JObject, JObjectArray, JString};
use jni::sys::jobject;
use jni::{jni_sig, jni_str, Env, EnvUnowned, JValue, JavaVM};

use crate::file_access::{ready_delivered, Delivered, FileAccess, PickKind, PickPurpose, PickRequest, Picked, NO_SHARE};
use crate::saf::{self, Dest, Pending};
use crate::system_clipboard::{ClipState, SystemClipboard};

/// Run `f` on the current thread, attached to the JVM, with the activity object.
fn with_activity<R>(f: impl FnOnce(&mut Env, &JObject) -> JniResult<R>) -> Result<R, String> {
    let ctx = ndk_context::android_context();
    if ctx.vm().is_null() || ctx.context().is_null() {
        return Err("The Java side is not available.".to_owned());
    }
    // Safety: ndk-context promises the VM pointer is valid for the life of the process.
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) };
    vm.attach_current_thread(|env| -> JniResult<R> {
        let raw: jobject = ctx.context().cast();
        // Safety: ndk-context's context is a global reference to the activity, kept alive by
        // android-activity. The cast does not take ownership, so it is never deleted here.
        let cast = unsafe { env.as_cast_raw::<JObject>(&raw)? };
        let activity: &JObject = &cast;
        f(env, activity)
    })
    .map_err(|e| e.to_string())
}

fn jstr<'l>(env: &mut Env<'l>, s: &str) -> JniResult<JString<'l>> {
    JString::new(env, s)
}

fn string_array<'l>(env: &mut Env<'l>, items: &[String]) -> JniResult<JObjectArray<'l, JString<'l>>> {
    let empty = JString::new(env, "")?;
    let arr = JObjectArray::<JString>::new(env, items.len(), &empty)?;
    for (i, item) in items.iter().enumerate() {
        let s = JString::new(env, item)?;
        arr.set_element(env, i, &s)?;
    }
    Ok(arr)
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// How long the export folder's name is reused: the Share tab asks every frame.
const TREE_MEMO: Duration = Duration::from_secs(2);

/// The storage-access backend: every `PickPurpose` through the system pickers.
pub struct SafFileAccess {
    cache: PathBuf,
    tree: Mutex<Option<(Instant, Option<String>)>>,
}

impl SafFileAccess {
    pub fn new() -> Self {
        let cache = with_activity(|env, a| {
            let s = env.call_method(a, jni_str!("cacheRoot"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
            let s = env.cast_local::<JString>(s)?;
            s.try_to_string(env)
        })
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
        Self { cache, tree: Mutex::new(None) }
    }

    /// The export folder's name while its permission is held (asks Java at most every 2 s).
    fn tree_name(&self, fresh: bool) -> Option<String> {
        let mut memo = self.tree.lock().unwrap_or_else(|e| e.into_inner());
        if !fresh {
            if let Some((at, name)) = memo.as_ref() {
                if at.elapsed() < TREE_MEMO {
                    return name.clone();
                }
            }
        }
        let name = with_activity(|env, a| {
            let s = env.call_method(a, jni_str!("treeState"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
            let s = env.cast_local::<JString>(s)?;
            s.try_to_string(env)
        })
        .ok()
        .filter(|s| !s.is_empty());
        *memo = Some((Instant::now(), name.clone()));
        name
    }

    fn forget_tree(&self) {
        *self.tree.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Answer `tx` with an error when a Java call failed, and drop the request.
    fn fail(req: i32, why: &str) {
        saf::global().take(req);
        saf::notice(why.to_owned());
    }
}

impl Default for SafFileAccess {
    fn default() -> Self {
        Self::new()
    }
}

impl FileAccess for SafFileAccess {
    fn pick(&self, req: PickRequest) -> Receiver<Picked> {
        let (tx, rx) = channel();
        let purpose = req.purpose;
        match req.kind {
            PickKind::OpenFile => {
                let cache = self.cache.clone();
                let id = saf::global().alloc_with(|id| Pending::Open { purpose, dir: saf::staging_dir(&cache, id), tx });
                let dir = path_str(&saf::staging_dir(&self.cache, id));
                let mimes: Vec<String> = saf::mimes_for(purpose).into_iter().map(str::to_owned).collect();
                log::info!("saf: pick open {purpose:?} req={id}");
                let r = with_activity(|env, a| {
                    let arr = string_array(env, &mimes)?;
                    let d = jstr(env, &dir)?;
                    env.call_method(
                        a,
                        jni_str!("pickOpen"),
                        jni_sig!("(I[Ljava/lang/String;Ljava/lang/String;)V"),
                        &[JValue::Int(id), JValue::Object(&arr), JValue::Object(&d)],
                    )?;
                    Ok(())
                });
                if let Err(e) = r {
                    Self::fail(id, &format!("The file picker could not be opened: {e}"));
                }
            }
            PickKind::SaveFile { suggested } => {
                let name = saf::clean_name(&suggested, "");
                if self.tree_name(true).is_some() {
                    // The export folder is chosen: no dialog, the file goes there when written.
                    let id = saf::global().fresh_id();
                    let dir = saf::staging_dir(&self.cache, id);
                    let _ = std::fs::create_dir_all(&dir);
                    saf::remember_stage(dir.clone(), Dest::Tree);
                    log::info!("saf: tree reused req={id}");
                    let _ = tx.send(Picked::Path(dir.join(name)));
                } else {
                    let cache = self.cache.clone();
                    let id = saf::global().alloc_with(|id| Pending::Create { dir: saf::staging_dir(&cache, id), tx });
                    let mime = saf::mimes_for(purpose).first().copied().unwrap_or("application/octet-stream");
                    log::info!("saf: pick create {purpose:?} req={id}");
                    let r = with_activity(|env, a| {
                        let m = jstr(env, mime)?;
                        let n = jstr(env, &name)?;
                        env.call_method(
                            a,
                            jni_str!("pickCreate"),
                            jni_sig!("(ILjava/lang/String;Ljava/lang/String;)V"),
                            &[JValue::Int(id), JValue::Object(&m), JValue::Object(&n)],
                        )?;
                        Ok(())
                    });
                    if let Err(e) = r {
                        Self::fail(id, &format!("The file picker could not be opened: {e}"));
                    }
                }
            }
            PickKind::Folder => match purpose {
                PickPurpose::ExportFolder if self.tree_name(true).is_some() => {
                    let id = saf::global().fresh_id();
                    let dir = saf::staging_dir(&self.cache, id);
                    let _ = std::fs::create_dir_all(&dir);
                    saf::remember_stage(dir.clone(), Dest::Tree);
                    log::info!("saf: tree reused req={id}");
                    let _ = tx.send(Picked::Path(dir));
                }
                PickPurpose::ExportFolder | PickPurpose::ExportTree => {
                    let cache = self.cache.clone();
                    let stage = purpose == PickPurpose::ExportFolder;
                    let id = saf::global().alloc_with(|id| {
                        if stage {
                            Pending::TreeStage { dir: saf::staging_dir(&cache, id), tx }
                        } else {
                            Pending::Tree { tx }
                        }
                    });
                    log::info!("saf: pick tree {purpose:?} req={id}");
                    self.forget_tree();
                    let r = with_activity(|env, a| {
                        env.call_method(a, jni_str!("pickTree"), jni_sig!("(I)V"), &[JValue::Int(id)])?;
                        Ok(())
                    });
                    if let Err(e) = r {
                        Self::fail(id, &format!("The folder picker could not be opened: {e}"));
                    }
                }
                _ => {
                    let _ = tx.send(Picked::Unavailable("Choose a place to save with the Share tab.".into()));
                }
            },
        }
        rx
    }

    fn commit(&self, written: &Path) -> Receiver<Delivered> {
        let Some(dest) = saf::dest_for(written) else {
            return ready_delivered(Delivered::Failed("This file was not saved through the file picker.".into()));
        };
        let (tx, rx) = channel();
        let id = saf::global().alloc(Pending::Commit { tx });
        let r = match dest {
            Dest::Doc(doc) => {
                let staged = path_str(written);
                log::info!("saf: commit file req={id} doc={doc}");
                with_activity(|env, a| {
                    let s = jstr(env, &staged)?;
                    env.call_method(
                        a,
                        jni_str!("commitFile"),
                        jni_sig!("(IILjava/lang/String;)V"),
                        &[JValue::Int(id), JValue::Int(doc), JValue::Object(&s)],
                    )?;
                    Ok(())
                })
            }
            Dest::Tree => {
                let (files, sub) = if written.is_dir() {
                    let mut v: Vec<String> = std::fs::read_dir(written)
                        .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file()).map(|p| path_str(&p)).collect())
                        .unwrap_or_default();
                    v.sort();
                    (v, written.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                } else {
                    (vec![path_str(written)], String::new())
                };
                log::info!("saf: commit into tree req={id} files={}", files.len());
                with_activity(|env, a| {
                    let arr = string_array(env, &files)?;
                    let s = jstr(env, &sub)?;
                    env.call_method(
                        a,
                        jni_str!("commitIntoTree"),
                        jni_sig!("(I[Ljava/lang/String;Ljava/lang/String;)V"),
                        &[JValue::Int(id), JValue::Object(&arr), JValue::Object(&s)],
                    )?;
                    Ok(())
                })
            }
        };
        if let Err(e) = r {
            saf::global().take(id);
            return ready_delivered(Delivered::Failed(e));
        }
        rx
    }

    fn share(&self, files: &[PathBuf], mime: &str) -> Result<(), String> {
        if files.is_empty() {
            return Err(NO_SHARE.to_owned());
        }
        let list: Vec<String> = files.iter().map(|f| path_str(f)).collect();
        with_activity(|env, a| {
            let arr = string_array(env, &list)?;
            let m = jstr(env, mime)?;
            let t = jstr(env, "Share")?;
            env.call_method(
                a,
                jni_str!("shareFiles"),
                jni_sig!("([Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"),
                &[JValue::Object(&arr), JValue::Object(&m), JValue::Object(&t)],
            )?;
            Ok(())
        })
    }

    fn share_path(&self, name: &str) -> Option<PathBuf> {
        static N: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let dir = saf::share_dir(&self.cache, stamp * 1000 + N.fetch_add(1, Ordering::SeqCst) % 1000);
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir.join(saf::clean_name(name, "")))
    }

    fn staged(&self) -> bool {
        true
    }

    fn export_folder(&self) -> Option<String> {
        self.tree_name(false)
    }
}

/// The system clipboard for images through `ClipboardManager`.
pub struct AndroidClipboard {
    cache: PathBuf,
}

impl AndroidClipboard {
    pub fn new() -> Self {
        let cache = with_activity(|env, a| {
            let s = env.call_method(a, jni_str!("cacheRoot"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
            let s = env.cast_local::<JString>(s)?;
            s.try_to_string(env)
        })
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
        Self { cache }
    }
}

impl Default for AndroidClipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemClipboard for AndroidClipboard {
    fn clip_dir(&self) -> Option<PathBuf> {
        Some(saf::clip_dir(&self.cache))
    }

    fn put_image(&self, png: &Path, serial: u64) {
        let p = path_str(png);
        let _ = with_activity(|env, a| {
            let s = jstr(env, &p)?;
            env.call_method(
                a,
                jni_str!("setClipImage"),
                jni_sig!("(Ljava/lang/String;J)V"),
                &[JValue::Object(&s), JValue::Long(serial as i64)],
            )?;
            Ok(())
        });
    }

    fn state(&self) -> ClipState {
        let text = with_activity(|env, a| {
            let s = env.call_method(a, jni_str!("clipState"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
            let s = env.cast_local::<JString>(s)?;
            s.try_to_string(env)
        })
        .unwrap_or_default();
        parse_clip_state(&text)
    }

    fn read_image(&self) -> Receiver<Result<PathBuf, String>> {
        let (tx, rx) = channel();
        let cache = self.cache.clone();
        let id = saf::global().alloc_with(|_| Pending::Clip { tx });
        let dir = path_str(&saf::staging_dir(&cache, id));
        let r = with_activity(|env, a| {
            let d = jstr(env, &dir)?;
            env.call_method(a, jni_str!("readClipImage"), jni_sig!("(ILjava/lang/String;)V"), &[JValue::Int(id), JValue::Object(&d)])?;
            Ok(())
        });
        if let Err(e) = r {
            if let Some(Pending::Clip { tx }) = saf::global().take(id) {
                let _ = tx.send(Err(e));
            }
        }
        rx
    }
}

/// `none`, `text`, `foreign` or `ours:<serial>` as `EskizActivity.clipState` answers.
pub fn parse_clip_state(text: &str) -> ClipState {
    match text {
        "text" => ClipState::TextOnly,
        "foreign" => ClipState::ForeignImage,
        t => t.strip_prefix("ours:").and_then(|n| n.parse().ok()).map_or(ClipState::None, ClipState::Ours),
    }
}

/// Tells Java whether Back has something to close.
pub struct AndroidBack;

impl crate::back::BackBridge for AndroidBack {
    fn set_wanted(&self, wanted: bool) {
        log::info!("back: wanted={wanted}");
        let _ = with_activity(|env, a| {
            env.call_method(a, jni_str!("setBackWanted"), jni_sig!("(Z)V"), &[JValue::Bool(wanted)])?;
            Ok(())
        });
    }
}

// ------------------------------------------------------------------------------------------------
// Java to Rust

/// The `Java_app_eskiz_EskizActivity_native*` functions of the A4 bridge. The method names and
/// signatures must match `EskizActivity.java`.
#[allow(unsafe_code, non_snake_case, clippy::too_many_arguments)]
mod jni_exports {
    use super::*;

    fn text(env: &Env, s: &JString) -> String {
        if s.is_null() {
            String::new()
        } else {
            s.try_to_string(env).unwrap_or_default()
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_app_eskiz_EskizActivity_nativePickResult<'a>(
        mut env: EnvUnowned<'a>,
        _class: JClass<'a>,
        req: i32,
        status: i32,
        path: JString<'a>,
        name: JString<'a>,
        mime: JString<'a>,
    ) {
        let (path, name, mime) = env
            .with_env(|env| -> JniResult<_> { Ok((text(env, &path), text(env, &name), text(env, &mime))) })
            .resolve::<jni::errors::LogErrorAndDefault>();
        log::info!("saf: result req={req} status={status} name={name}");
        let dest = saf::global().resolve_pick(req, status, &path, &name, &mime);
        if let Some((dir, dest)) = dest {
            saf::remember_stage(dir, dest);
        }
        crate::android_queue::wake();
    }

    #[no_mangle]
    pub extern "system" fn Java_app_eskiz_EskizActivity_nativeCommitResult<'a>(
        mut env: EnvUnowned<'a>,
        _class: JClass<'a>,
        req: i32,
        status: i32,
        message: JString<'a>,
    ) {
        let message = env.with_env(|env| -> JniResult<_> { Ok(text(env, &message)) }).resolve::<jni::errors::LogErrorAndDefault>();
        saf::global().resolve_commit(req, status, &message);
        crate::android_queue::wake();
    }

    #[no_mangle]
    pub extern "system" fn Java_app_eskiz_EskizActivity_nativeClipResult<'a>(
        mut env: EnvUnowned<'a>,
        _class: JClass<'a>,
        req: i32,
        status: i32,
        path: JString<'a>,
        message: JString<'a>,
    ) {
        let (path, message) = env
            .with_env(|env| -> JniResult<_> { Ok((text(env, &path), text(env, &message))) })
            .resolve::<jni::errors::LogErrorAndDefault>();
        saf::global().resolve_clip(req, status, &path, &message);
        crate::android_queue::wake();
    }

    #[no_mangle]
    pub extern "system" fn Java_app_eskiz_EskizActivity_nativeBack<'a>(_env: EnvUnowned<'a>, _class: JClass<'a>) {
        saf::push_back();
        crate::android_queue::wake();
    }

    #[no_mangle]
    pub extern "system" fn Java_app_eskiz_EskizActivity_nativeTestHook<'a>(mut env: EnvUnowned<'a>, _class: JClass<'a>, hook: JString<'a>) {
        let hook = env.with_env(|env| -> JniResult<_> { Ok(text(env, &hook)) }).resolve::<jni::errors::LogErrorAndDefault>();
        saf::push_hook(hook);
        crate::android_queue::wake();
    }

    #[no_mangle]
    pub extern "system" fn Java_app_eskiz_EskizActivity_nativeNotice<'a>(mut env: EnvUnowned<'a>, _class: JClass<'a>, message: JString<'a>) {
        let message = env.with_env(|env| -> JniResult<_> { Ok(text(env, &message)) }).resolve::<jni::errors::LogErrorAndDefault>();
        saf::notice(message);
        crate::android_queue::wake();
    }
}
