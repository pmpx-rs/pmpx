//! pmpx —— 一个命令面，识别项目类型，转发给真正的工具。
//!
//! 这个二进制里没有一行插件代码：支持 cargo / pnpm 的能力都在各自的插件仓库里，
//! 运行时 `dlopen` 进来。
//!
//! # 分层
//! ```text
//! main.rs / cli.rs                    进程边界与参数解析
//! commands.rs / app.rs                子命令处理与流程编排
//! config.rs / discovery.rs            配置读取与项目根发现
//! plugins.rs / detect.rs / hints.rs   插件清单、裁决、零插件时的提示
//! runtime.rs / spawn.rs               加载选中的插件、启动进程
//! ```
//!
//! 一条硬规矩：检测阶段绝不加载任何插件代码（`plugins.rs` 只读 manifest）—— 在一个
//! 从没装过插件的仓库里跑 `pmpx`，它不会执行任何第三方代码。

#![deny(missing_docs)]
#![warn(clippy::all)]

mod app;
mod cli;
mod commands;
mod config;
mod detect;
mod discovery;
mod error;
mod hints;
mod plugins;
mod runtime;
mod spawn;

use clap::Parser;

fn main() -> std::process::ExitCode {
    let args = cli::Cli::parse();
    let code = dispatch(&args);
    std::process::ExitCode::from(code)
}

/// 跑一次，返回进程退出码 —— 可能来自后端。
fn dispatch(args: &cli::Cli) -> u8 {
    match commands::dispatch(args) {
        Ok(code) => code,
        Err(e) => {
            // 统一前缀，让用户在后端输出里一眼分辨出哪些话是 pmpx 说的。
            eprintln!("pmpx: {e}");
            e.exit_code()
        }
    }
}
