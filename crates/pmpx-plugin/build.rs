//! 把编译期的 rustc 版本与 target triple 注入进去，供 `pmpx_plugin::abi` 填进
//! `PmpxPluginV1`。
//!
//! `rustc --version` 是**运行期**问不到的东西，所以编译期问一次、把答案固化进二进制。
//! 这些常量会被链接进每一个插件，插件因此不需要自己的 build.rs。

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // 换工具链要重新跑，否则会把上一个工具链的版本号固化进去。
    println!("cargo:rerun-if-env-changed=RUSTC");

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());

    // 问不到就写 "unknown" —— 这个字段只用于诊断展示，拿不到不该让构建失败。
    let version = Command::new(&rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=PMPX_BUILD_RUSTC={version}");
    println!("cargo:rustc-env=PMPX_BUILD_TARGET={target}");
}
