//! [`export!`](macro@crate::export) —— 把 [`PackageManager`](crate::PackageManager)
//! 实现变成一整套 C ABI 外壳。

/// 生成插件的整个 C ABI 外壳：入口符号，以及 `name` / `family` / `command` / `free_*` 各 shim。
///
/// 用法是一行；`create` 必须是一个返回 `Box<dyn PackageManager>` 的**函数的路径**，不能传
/// 类型名，因为你可能想在里面做构造参数注入：
/// ```ignore
/// pub fn create() -> Box<dyn pmpx_plugin::PackageManager> { Box::new(MyPlugin) }
/// pmpx_plugin::export!(create);
/// ```
///
/// 这个形状由 [`crate-plugin-kit`] 生成的 wrapper 工程约定，两边必须对得上。
///
/// 每次调用都新建实例，刻意不缓存 —— 宿主一个进程只调 `name` / `family` / `command` 各一次。
///
/// [`crate-plugin-kit`]: https://crates.io/crates/crate-plugin-kit
#[macro_export]
macro_rules! export {
    ($create:path) => {
        #[doc(hidden)]
        fn __pmpx_instance() -> ::std::boxed::Box<dyn $crate::PackageManager> {
            $create()
        }

        #[doc(hidden)]
        unsafe extern "C" fn __pmpx_name() -> $crate::abi::PmpxStr {
            $crate::abi::leak_str(__pmpx_instance().name())
        }

        #[doc(hidden)]
        unsafe extern "C" fn __pmpx_family() -> $crate::abi::PmpxStr {
            $crate::abi::leak_str(__pmpx_instance().family().as_str())
        }

        #[doc(hidden)]
        unsafe extern "C" fn __pmpx_command(
            project_root: $crate::abi::PmpxStr,
            matched: *const $crate::abi::PmpxStr,
            matched_len: ::std::primitive::usize,
            verb: ::std::primitive::u32,
            args: *const $crate::abi::PmpxStr,
            args_len: ::std::primitive::usize,
            out: *mut $crate::abi::PmpxCommand,
        ) -> ::std::primitive::u32 {
            // `guard` 不是可选项：panic 越过 `extern "C"` 边界会直接 abort，宿主救不了。
            $crate::abi::guard(move || {
                let plugin = __pmpx_instance();
                // SAFETY: 参数与 out 的有效性由调用方（宿主）按 `PmpxPluginV1::command`
                // 的约定保证；这里只是把它们转交给同一份约定下的实现。
                unsafe {
                    $crate::abi::dispatch_command(
                        &*plugin,
                        project_root,
                        matched,
                        matched_len,
                        verb,
                        args,
                        args_len,
                        out,
                    )
                }
            })
        }

        #[doc(hidden)]
        unsafe extern "C" fn __pmpx_free_str(s: $crate::abi::PmpxStr) {
            // SAFETY: 这个函数只会被宿主拿 vtable 里的指针调，而 vtable 只由本宏产出，
            // 所以传回来的必然是本侧 leak_* 的成果。
            unsafe { $crate::abi::free_str(s) }
        }

        #[doc(hidden)]
        unsafe extern "C" fn __pmpx_free_command(c: *mut $crate::abi::PmpxCommand) {
            // SAFETY: 同上 —— 只有本侧 write_command 填充过的结构体会走到这里。
            unsafe { $crate::abi::free_command(c) }
        }

        #[doc(hidden)]
        static __PMPX_ENTRY: $crate::abi::PmpxPluginV1 = $crate::abi::PmpxPluginV1 {
            abi_version: $crate::abi::ABI_VERSION,
            rustc_version: $crate::abi::build_rustc(),
            target: $crate::abi::build_target(),
            name: __pmpx_name,
            family: __pmpx_family,
            command: __pmpx_command,
            free_str: __pmpx_free_str,
            free_command: __pmpx_free_command,
        };

        /// 插件的唯一入口符号。由 `pmpx_plugin::export!` 生成，**不要手写**。
        /// 用 `#[unsafe(no_mangle)]` 而不是 `#[no_mangle]`：后者在 edition 2024 里是硬错误。
        #[unsafe(no_mangle)]
        pub extern "C" fn pmpx_plugin_entry_v1() -> *const $crate::abi::PmpxPluginV1 {
            &__PMPX_ENTRY
        }
    };
}
