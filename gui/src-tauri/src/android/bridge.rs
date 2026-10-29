use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::sys::{jboolean, jstring};
use jni::{JNIEnv, JavaVM};

pub(super) struct Bridge {
    vm: JavaVM,
    runtime: GlobalRef,
}

static BRIDGE: OnceLock<Arc<Bridge>> = OnceLock::new();

pub(crate) fn joining(joining: bool) -> Result<(), String> {
    let bridge = BRIDGE.get().ok_or("Android has not started its native bridge")?;
    bridge.action("setJoining", if joining { "true" } else { "false" })?;
    if !joining {
        let world = super::world()?;
        let state = serde_json::to_string(&world.device().expect("Android device").state())
            .map_err(|error| error.to_string())?;
        bridge.action("publishState", &state)?;
    }
    Ok(())
}

/// A login is over, however it went. Should telling Android fail, the
/// background connection still follows the device's state, which the
/// monitor publishes every two seconds; the login stands.
pub(crate) fn joining_ended() {
    if let Err(error) = joining(false) {
        report(format!("telling Android a login ended: {error}"));
    }
}

impl Bridge {
    fn with<T>(&self, call: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> jni::errors::Result<T>) -> Result<T, String> {
        let mut env = self.vm.attach_current_thread().map_err(|error| error.to_string())?;
        match call(&mut env, self.runtime.as_obj()) {
            Ok(value) => Ok(value),
            Err(error) => {
                if let Ok(exception) = env.exception_occurred()
                    && !exception.is_null()
                {
                    env.exception_clear().map_err(|error| error.to_string())?;
                    let message = env
                        .call_method(exception, "toString", "()Ljava/lang/String;", &[])
                        .and_then(|value| value.l())
                        .and_then(|value| env.get_string(&JString::from(value)).map(Into::into));
                    return Err(message.unwrap_or_else(|_| error.to_string()));
                }
                Err(error.to_string())
            }
        }
    }

    pub fn string(&self, method: &str) -> Result<String, String> {
        self.with(|env, object| {
            let value = env.call_method(object, method, "()Ljava/lang/String;", &[])?.l()?;
            env.get_string(&JString::from(value)).map(Into::into)
        })
    }

    pub fn string_arg(&self, method: &str, argument: &str) -> Result<String, String> {
        self.with(|env, object| {
            let argument = env.new_string(argument)?;
            let value = env
                .call_method(object, method, "(Ljava/lang/String;)Ljava/lang/String;", &[JValue::Object(&argument)])?
                .l()?;
            env.get_string(&JString::from(value)).map(Into::into)
        })
    }

    pub fn action(&self, method: &str, argument: &str) -> Result<(), String> {
        self.with(|env, object| {
            let argument = env.new_string(argument)?;
            env.call_method(object, method, "(Ljava/lang/String;)V", &[JValue::Object(&argument)])?;
            Ok(())
        })
    }

    pub fn focused(&self) -> Result<bool, String> {
        self.with(|env, object| env.call_method(object, "isFocused", "()Z", &[])?.z())
    }
}

impl fastctx_net::android::Platform for Bridge {
    fn choices(&self, target: SocketAddr) -> Result<fastctx_net::android::Choices, String> {
        serde_json::from_str(&self.string_arg("networkChoices", &target.to_string())?)
            .map_err(|error| error.to_string())
    }
}

pub(super) fn report(error: String) {
    if let Some(bridge) = BRIDGE.get()
        && let Err(native) = bridge.action("reportError", &error)
    {
        eprintln!("{error}; reporting it to Android: {native}");
    }
}

fn result(env: &mut JNIEnv<'_>, error: Option<String>) -> jstring {
    match error {
        None => std::ptr::null_mut(),
        Some(error) => match env.new_string(error) {
            Ok(text) => text.into_raw(),
            Err(error) => {
                let _ = env.throw_new("java/lang/IllegalStateException", error.to_string());
                std::ptr::null_mut()
            }
        },
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fastctx_world_WorldRuntime_startNative(
    mut env: JNIEnv<'_>,
    object: JObject<'_>,
    directory: JString<'_>,
) -> jstring {
    let start = (|| {
        let directory: String = env.get_string(&directory).map_err(|error| error.to_string())?.into();
        let bridge = Arc::new(Bridge {
            vm: env.get_java_vm().map_err(|error| error.to_string())?,
            runtime: env.new_global_ref(object).map_err(|error| error.to_string())?,
        });
        super::start(directory.into(), bridge.clone())?;
        // Kept once started; one started before stays.
        let _ = BRIDGE.set(bridge);
        Ok::<(), String>(())
    })();
    result(&mut env, start.err())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fastctx_world_WorldRuntime_reconnectNative(_: JNIEnv<'_>, _: JObject<'_>) {
    super::reconnect();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fastctx_world_WorldRuntime_activityNative(
    _: JNIEnv<'_>,
    _: JObject<'_>,
    ready: jboolean,
) {
    super::activity(ready != 0);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fastctx_world_WorldRuntime_visibleNative(
    _: JNIEnv<'_>,
    _: JObject<'_>,
    visible: jboolean,
) {
    super::visible(visible != 0);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fastctx_world_WorldRuntime_wakePageNative(_: JNIEnv<'_>, _: JObject<'_>) {
    super::pending();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fastctx_world_WorldRuntime_validateConfigurationNative(
    mut env: JNIEnv<'_>,
    _: JObject<'_>,
    json: JString<'_>,
) -> jstring {
    let parsed = (|| {
        let text: String = env.get_string(&json).map_err(|error| error.to_string())?.into();
        super::observer::validate(&text)
    })();
    result(&mut env, parsed.err())
}
