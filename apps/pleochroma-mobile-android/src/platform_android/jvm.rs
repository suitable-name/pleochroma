//! Attaching to the JVM and calling the Kotlin `MainActivity`.
//!
//! Rust holds one global reference to the activity (NativeActivity's Java object *is*
//! our `MainActivity` instance) and calls its methods by name. Most Kotlin service methods
//! return a `String?`: `null` on success, a sentence for the user on failure. The query
//! methods (`thermalHeadroom`, `systemPowerSave`, `networkMetered`, `cacheDirPath`) return
//! their value, and `loadRemoteCredentials` returns the stored bundle. A Java exception
//! that escapes is turned into the same kind of message by `describe`.

use jni::{
    Env, JValueOwned,
    errors::{Error as JniError, Result as JniResult},
    jni_sig,
    objects::{JByteArray, JObject, JString},
    refs::{Global, Reference},
    signature::MethodSignature,
    sys::{jobject, jsize},
    vm::JavaVM,
};
use pleochroma_mobile_core::platform::{DeviceFacts, ExitInfo, ExitReason, PlatformError};
use slint::android::AndroidApp;

/// How many bytes cross the JNI boundary per region call. Bounds the temporary
/// `i8` buffer used to convert between `u8` and Java's signed bytes.
const CHUNK: usize = 1 << 20;

/// `openDocuments(requestId: Long, kind: Int, allowMultiple: Boolean): String?`
pub(super) const SIG_OPEN_DOCUMENTS: MethodSignature = jni_sig!("(JIZ)Ljava/lang/String;");
/// `createDocument(requestId: Long, name: String, mime: String, bytes: ByteArray): String?`
pub(super) const SIG_CREATE_DOCUMENT: MethodSignature =
    jni_sig!("(JLjava/lang/String;Ljava/lang/String;[B)Ljava/lang/String;");
/// `share(name: String, mime: String, bytes: ByteArray): String?`
pub(super) const SIG_SHARE: MethodSignature =
    jni_sig!("(Ljava/lang/String;Ljava/lang/String;[B)Ljava/lang/String;");
/// `printHtml(title: String, html: String): String?`
pub(super) const SIG_PRINT_HTML: MethodSignature =
    jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;");
/// `setClipboardText(text: String): String?` and `openUrl(url: String): String?`
pub(super) const SIG_ONE_STRING: MethodSignature =
    jni_sig!("(Ljava/lang/String;)Ljava/lang/String;");
/// `keepScreenOn(on: Boolean): String?`
pub(super) const SIG_KEEP_SCREEN_ON: MethodSignature = jni_sig!("(Z)Ljava/lang/String;");
/// `thermalHeadroom(): Float`
pub(super) const SIG_THERMAL_HEADROOM: MethodSignature = jni_sig!("()F");
/// `systemPowerSave(): Boolean`
pub(super) const SIG_SYSTEM_POWER_SAVE: MethodSignature = jni_sig!("()Z");
/// `networkMetered(): Int` (0 unmetered, 1 metered, -1 unknown)
pub(super) const SIG_NETWORK_METERED: MethodSignature = jni_sig!("()I");
/// `cacheDirPath(): String`
pub(super) const SIG_CACHE_DIR: MethodSignature = jni_sig!("()Ljava/lang/String;");
/// `loadRemoteCredentials(): String?` and `forgetRemoteCredentials(): String?`; also
/// `deviceFacts(): String`
pub(super) const SIG_NO_ARGS_STRING: MethodSignature = jni_sig!("()Ljava/lang/String;");
/// `lastExitInfo(afterMs: Long): String?`
pub(super) const SIG_LAST_EXIT: MethodSignature = jni_sig!("(J)Ljava/lang/String;");
/// `composeEmail(to: String, subject: String, body: String, name: String, bytes: ByteArray): String?`
pub(super) const SIG_COMPOSE_EMAIL: MethodSignature = jni_sig!(
    "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[B)Ljava/lang/String;"
);

/// The JVM and the activity, usable from any thread.
pub(super) struct Jvm {
    vm: JavaVM,
    activity: Global<JObject<'static>>,
}

impl Jvm {
    /// Captures the process-wide JVM and a global reference to the activity.
    pub(super) fn new(app: &AndroidApp) -> Result<Self, PlatformError> {
        // SAFETY: `vm_as_ptr` is the process's `JavaVM*`, valid for the life of the
        // process (android-activity documents this exact use). `from_raw` stores it as
        // the crate's singleton, or returns the one already stored.
        let vm = JavaVM::singleton()
            .unwrap_or_else(|_| unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) });
        let activity = vm
            .attach_current_thread(|env| -> JniResult<Global<JObject<'static>>> {
                let raw: jobject = app.activity_as_ptr().cast();
                // SAFETY: `activity_as_ptr` is a global reference owned by the
                // android-activity glue that outlives this call. `Cast` borrows it
                // without taking ownership, so it is never deleted here.
                let borrowed = unsafe { env.as_cast_raw::<JObject>(&raw)? };
                env.new_global_ref(&*borrowed)
            })
            .map_err(|error| {
                PlatformError::new(format!(
                    "Could not reach the Android activity: {}",
                    describe(&error)
                ))
            })?;
        Ok(Self { vm, activity })
    }

    /// Attaches the calling thread (if needed) and runs `f` with the environment and
    /// the activity.
    pub(super) fn with_env<R>(
        &self,
        f: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> JniResult<R>,
    ) -> Result<R, PlatformError> {
        self.vm
            .attach_current_thread(|env| f(env, self.activity.as_obj()))
            .map_err(|error| PlatformError::new(describe(&error)))
    }
}

/// A user-readable sentence for a JNI failure; a Java exception contributes its own
/// message.
fn describe(error: &JniError) -> String {
    match error {
        JniError::CaughtJavaException { msg, name, .. } => {
            if msg.is_empty() {
                name.clone()
            } else {
                msg.clone()
            }
        }
        other => other.to_string(),
    }
}

/// Reads the `String?` a Kotlin service method returned: `None` for `null`.
pub(super) fn error_message(
    env: &mut Env<'_>,
    value: JValueOwned<'_>,
) -> JniResult<Option<String>> {
    let object = value.l()?;
    if object.is_null() {
        return Ok(None);
    }
    let text = env.cast_local::<JString>(object)?;
    Ok(Some(text.try_to_string(env)?))
}

/// Turns the result of a Kotlin service call (`None` = success) into the platform
/// result.
pub(super) fn into_unit(
    result: Result<Option<String>, PlatformError>,
) -> Result<(), PlatformError> {
    match result? {
        None => Ok(()),
        Some(message) => Err(PlatformError::new(message)),
    }
}

/// The `key=value` lines of a Kotlin answer; a line without `=` is skipped.
fn key_values(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines().filter_map(|line| line.split_once('='))
}

/// Reads the answer of `MainActivity.deviceFacts`. A missing or malformed value stays at its
/// default; the thermal headroom is filled in by the caller.
pub(super) fn parse_device_facts(text: &str) -> DeviceFacts {
    let mut facts = DeviceFacts::default();
    for (key, value) in key_values(text) {
        match key {
            "app_version" => facts.app_version = value.to_owned(),
            "android_release" => facts.android_release = value.to_owned(),
            "sdk_int" => facts.sdk_int = value.parse().unwrap_or(0),
            "manufacturer" => facts.manufacturer = value.to_owned(),
            "model" => facts.model = value.to_owned(),
            "soc_model" => facts.soc_model = (!value.is_empty()).then(|| value.to_owned()),
            "abi" => facts.abi = value.to_owned(),
            "total_ram_mb" => facts.total_ram_mb = value.parse().unwrap_or(0),
            "fingerprint" => facts.fingerprint = value.to_owned(),
            _ => {}
        }
    }
    facts
}

/// Reads the answer of `MainActivity.lastExitInfo`. `None` when the reason or the time is
/// missing.
pub(super) fn parse_exit_info(text: &str) -> Option<ExitInfo> {
    let mut reason = None;
    let mut description = String::new();
    let mut timestamp_ms = None;
    let mut pid = 0_u32;
    let mut trace_file = None;
    for (key, value) in key_values(text) {
        match key {
            "reason" => {
                reason = Some(match value {
                    "crash_native" => ExitReason::CrashNative,
                    "crash" => ExitReason::Crash,
                    "anr" => ExitReason::Anr,
                    "user_requested" => ExitReason::UserRequested,
                    "low_memory" => ExitReason::LowMemory,
                    _ => ExitReason::Other,
                });
            }
            "description" => description = value.to_owned(),
            "timestamp_ms" => timestamp_ms = value.parse::<i64>().ok(),
            "pid" => pid = value.parse().unwrap_or(0),
            "trace_file" => trace_file = (!value.is_empty()).then(|| value.to_owned()),
            _ => {}
        }
    }
    Some(ExitInfo {
        reason: reason?,
        description,
        timestamp_ms: timestamp_ms?,
        pid,
        trace_file,
    })
}

/// Copies `bytes` into a new Java `byte[]`.
pub(super) fn new_byte_array<'local>(
    env: &mut Env<'local>,
    bytes: &[u8],
) -> JniResult<JByteArray<'local>> {
    let array = JByteArray::new(env, bytes.len())?;
    let mut signed: Vec<i8> = Vec::with_capacity(CHUNK.min(bytes.len()));
    let mut start = 0_usize;
    for piece in bytes.chunks(CHUNK) {
        signed.clear();
        signed.extend(piece.iter().map(|&byte| byte as i8));
        array.set_region(env, start as jsize, &signed)?;
        start += piece.len();
    }
    Ok(array)
}

/// Copies a Java `byte[]` into a `Vec<u8>`.
pub(super) fn read_byte_array(env: &Env<'_>, array: &JByteArray<'_>) -> JniResult<Vec<u8>> {
    let len = array.len(env)?;
    let mut out = Vec::with_capacity(len);
    let mut signed = vec![0_i8; CHUNK.min(len)];
    let mut start = 0_usize;
    while start < len {
        let count = CHUNK.min(len - start);
        array.get_region(env, start as jsize, &mut signed[..count])?;
        out.extend(signed[..count].iter().map(|&byte| byte as u8));
        start += count;
    }
    Ok(out)
}
