use std::ffi::c_void;
use std::sync::OnceLock;

use jni::objects::{GlobalRef, JClass, JObject};
use jni::sys::{jboolean, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;
use tracing_subscriber::layer::SubscriberExt;

static ANDROID_CONTEXT: OnceLock<GlobalRef> = OnceLock::new();
static ANDROID_TRACING_INSTALLED: OnceLock<()> = OnceLock::new();

fn install_android_tracing() {
    ANDROID_TRACING_INSTALLED.get_or_init(|| {
        let Ok(layer) = tracing_android::layer("UcEngine") else {
            return;
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_ucengine_UcEngineModule_nativeInstallAndroidContext(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    context: JObject<'_>,
) -> jboolean {
    install_android_tracing();
    let vm = match env.get_java_vm() {
        Ok(vm) => vm,
        Err(_) => return JNI_FALSE,
    };
    let context = match env.new_global_ref(context) {
        Ok(context) => context,
        Err(_) => return JNI_FALSE,
    };

    ANDROID_CONTEXT.get_or_init(|| {
        unsafe {
            ndk_context::initialize_android_context(
                vm.get_java_vm_pointer().cast::<c_void>(),
                context.as_obj().as_raw().cast::<c_void>(),
            );
        }
        context
    });

    JNI_TRUE
}
