//! `export!`: the whole C shell, generated.
//!
//! A plugin author writes one line:
//!
//! ```ignore
//! pub fn create() -> Box<dyn PackageManager> { Box::new(CargoPlugin) }
//! pmpx_plugin::export!(create);
//! ```
//!
//! and this macro produces everything the host looks for: the `identity`, `command` and `attach`
//! capability tables, the root table, the capability lookup, and the one exported symbol. None of it
//! is something an author should have to see, and none of it is a place for a plugin to make a
//! decision -- it is marshalling, panic containment and ownership, exactly as the ABI describes.
//!
//! # What the generated code promises the host
//!
//! - **A panic never crosses.** Every entry point is wrapped in `catch_unwind`; a panic in `name`,
//!   `family` or `command` becomes the contract's marker or an error code, never an abort.
//! - **Memory is freed by the side that allocated it.** What the shells lease out is reclaimed by
//!   the `free_str` / `free_command` shims, and never by the host.
//! - **A verb this build does not know is `unsupported`**, not "invalid arguments": that is what
//!   keeps a new verb additive for the host's degradation path.

/// Generate the C shell for one [`PackageManager`](crate::PackageManager) implementation.
#[macro_export]
macro_rules! export {
    ($create:expr) => {
        /// The plugin's factory, wrapped.
        fn __pmpx_instance() -> ::std::boxed::Box<dyn $crate::PackageManager> {
            let create: fn() -> ::std::boxed::Box<dyn $crate::PackageManager> = $create;
            create()
        }

        #[doc(hidden)]
        unsafe extern "C" fn __pmpx_name() -> $crate::abi::PmpxStr {
            $crate::shell::guard_str(|| $crate::shell::leak_str(__pmpx_instance().name()))
        }

        #[doc(hidden)]
        unsafe extern "C" fn __pmpx_family() -> $crate::abi::PmpxStr {
            $crate::shell::guard_str(|| {
                $crate::shell::leak_str(__pmpx_instance().family().as_str())
            })
        }

        #[doc(hidden)]
        unsafe extern "C" fn __pmpx_run(
            context: *const $crate::abi::PmpxContext,
            out: *mut $crate::abi::PmpxCommand,
        ) -> ::std::primitive::u32 {
            // `guard` is not optional: a panic crossing the `extern "C"` boundary aborts the whole
            // process, and the host cannot save it.
            $crate::shell::guard(move || {
                let plugin = __pmpx_instance();
                // Remember the plugin's own name for the case where no host installed hooks (a
                // plugin's own tests); the host path never needs it, since the host knows its id.
                $crate::shell::remember_name(plugin.name());
                // SAFETY: the host promises the context and `out` under the `command` contract.
                unsafe { $crate::shell::dispatch(&*plugin, context, out) }
            })
        }

        #[doc(hidden)]
        unsafe extern "C" fn __pmpx_attach(host: *const $crate::abi::PmpxHost) {
            if host.is_null() {
                return;
            }

            // SAFETY: the host hands over its own table, valid for the process.
            let lookup = unsafe { (*host).capability };
            let key = $crate::abi::PmpxStr::new(
                $crate::abi::PMPX_CAP_LOG.as_ptr(),
                $crate::abi::PMPX_CAP_LOG.len(),
            );
            // SAFETY: a borrow that outlives the call.
            let table = unsafe { lookup(key) };
            if table.is_null() {
                return;
            }

            let table = table as *const $crate::abi::PmpxLog;
            // Read no further than the host says it built: a host with an older, smaller table is
            // "no hooks" rather than a read past its end.
            // SAFETY: the pointer is non-null and belongs to the host.
            if unsafe { (*table).size } < ::std::mem::size_of::<$crate::abi::PmpxLog>() {
                return;
            }

            // SAFETY: checked above, and the host keeps it alive.
            unsafe { $crate::shell::install_log_table(table) };
        }

        #[doc(hidden)]
        unsafe extern "C" fn __pmpx_free_str(s: $crate::abi::PmpxStr) {
            // SAFETY: the host only passes back what this side leased out.
            unsafe { $crate::shell::dispatch_free_str(s) }
        }

        #[doc(hidden)]
        unsafe extern "C" fn __pmpx_free_command(command: *mut $crate::abi::PmpxCommand) {
            // SAFETY: as above.
            unsafe { $crate::shell::dispatch_free_command(command) }
        }

        #[doc(hidden)]
        unsafe extern "C" fn __pmpx_capability(
            name: $crate::abi::PmpxStr,
        ) -> *const ::std::ffi::c_void {
            // SAFETY: the host passes a borrow that outlives the call.
            let Some(name) = (unsafe { $crate::shell::read_str(name) }) else {
                return ::std::ptr::null();
            };

            match name {
                $crate::abi::PMPX_CAP_IDENTITY => ::std::ptr::from_ref(&__PMPX_IDENTITY).cast(),
                $crate::abi::PMPX_CAP_COMMAND => ::std::ptr::from_ref(&__PMPX_COMMAND).cast(),
                $crate::abi::PMPX_CAP_ATTACH => ::std::ptr::from_ref(&__PMPX_ATTACH).cast(),
                // A capability this build does not have is "not here", which is what lets a host
                // ask for one it knows and this plugin keep working.
                _ => ::std::ptr::null(),
            }
        }

        #[doc(hidden)]
        static __PMPX_IDENTITY: $crate::abi::PmpxIdentity = $crate::abi::PmpxIdentity {
            size: ::std::mem::size_of::<$crate::abi::PmpxIdentity>(),
            name: __pmpx_name,
            family: __pmpx_family,
            free_str: __pmpx_free_str,
        };

        #[doc(hidden)]
        static __PMPX_COMMAND: $crate::abi::PmpxCommandCap = $crate::abi::PmpxCommandCap {
            size: ::std::mem::size_of::<$crate::abi::PmpxCommandCap>(),
            run: __pmpx_run,
            free_command: __pmpx_free_command,
        };

        #[doc(hidden)]
        static __PMPX_ATTACH: $crate::abi::PmpxAttach = $crate::abi::PmpxAttach {
            size: ::std::mem::size_of::<$crate::abi::PmpxAttach>(),
            attach: __pmpx_attach,
        };

        #[doc(hidden)]
        static __PMPX_PLUGIN: $crate::abi::PmpxPlugin = $crate::abi::PmpxPlugin {
            abi_major: $crate::abi::PMPX_ABI_MAJOR,
            rustc_version: $crate::abi::PmpxStr::new(
                $crate::shell::BUILD_RUSTC.as_ptr(),
                $crate::shell::BUILD_RUSTC.len(),
            ),
            target: $crate::abi::PmpxStr::new(
                $crate::shell::BUILD_TARGET.as_ptr(),
                $crate::shell::BUILD_TARGET.len(),
            ),
            capability: __pmpx_capability,
        };

        // Compile-time proof that the shell's shape is the shape the table declares: if the two ever
        // drift, this stops compiling instead of becoming a call through the wrong function type --
        // which the ABI's major version could not catch, because it would not change.
        const _: $crate::abi::RunFn = __pmpx_run;

        /// The one symbol a host looks for.
        ///
        /// The version in the name tracks the *root table's layout*, not the keys: a host that finds
        /// no such symbol says "built against another contract" instead of reading fields that
        /// moved.
        #[unsafe(no_mangle)]
        pub extern "C" fn pmpx_plugin_entry_v3() -> *const $crate::abi::PmpxPlugin {
            &__PMPX_PLUGIN
        }
    };
}
