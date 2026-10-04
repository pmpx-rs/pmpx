//! 子命令的处理。
//!
//! 这一层只做"读上下文 → 做事 → 打印"，判断逻辑在 [`crate::app`] 与它下面。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use clap::CommandFactory;
use pmpx_plugin::Family;

use crate::app::{self, Session};
use crate::cli::{Cli, Command, ConfigCommand, PluginCommand};
use crate::config::ProjectConfig;
use crate::detect::{self, FamilyScore, ScoredPlugin};
use crate::error::{PmpxError, EXIT_OK};

/// 按 argv 分发。
pub fn dispatch(args: &Cli) -> crate::error::Result<u8> {
    let Some(command) = &args.command else {
        return show_detection(args);
    };

    match command {
        Command::Install { packages } => {
            run_verb(args, pmpx_plugin::Verb::Install, packages.clone(), false)
        }
        Command::Remove { packages } => {
            run_verb(args, pmpx_plugin::Verb::Remove, packages.clone(), false)
        }
        Command::Update { packages } => {
            run_verb(args, pmpx_plugin::Verb::Update, packages.clone(), false)
        }
        Command::Build { args: rest } => {
            run_verb(args, pmpx_plugin::Verb::Build, rest.clone(), false)
        }
        Command::Test { args: rest } => {
            run_verb(args, pmpx_plugin::Verb::Test, rest.clone(), false)
        }

        Command::Run { target, args: rest } => {
            // `pmpx run <target> -- <args>`：target 与 `--` 之后的内容一起交给插件，
            // 怎么摆由插件自己决定（例如 cargo 会用 `cargo run -- …`）。
            let argv: Vec<OsString> = target.iter().cloned().chain(rest.iter().cloned()).collect();
            run_verb(args, pmpx_plugin::Verb::Run, argv, false)
        }

        // `exec` 是唯一允许降级的动词
        Command::Exec { command } => run_verb(args, pmpx_plugin::Verb::Exec, command.clone(), true),

        Command::Info => info(args),

        Command::Plugin(sub) => plugin_cmd(args, sub),
        Command::Config(sub) => config_cmd(sub),
        Command::Completion { shell } => completion(*shell),
    }
}

/// 统一的动词入口：开 session，交给 [`app::run_verb`]。
fn run_verb(
    args: &Cli,
    verb: pmpx_plugin::Verb,
    argv: Vec<OsString>,
    allow_exec_fallback: bool,
) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;
    app::run_verb(&session, verb, &argv, allow_exec_fallback)
}

// ---------------------------------------------------------------------------
// 不带子命令：显示检测结果
// ---------------------------------------------------------------------------

/// 裸 `pmpx`：说清"这个目录是什么、会走哪个后端"。
fn show_detection(args: &Cli) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    let Some(root) = session.project_root().map(PathBuf::from) else {
        return Err(session.no_project_error());
    };

    let selection = session.select(&root).map_err(PmpxError::from)?;

    session.emit_notes(&selection);

    println!("项目根   {}", root.display());
    println!("生态     {}", selection.family.display());
    println!("插件     {}（{} 分）", selection.name, selection.score);
    println!();
    println!("用 `pmpx info` 看全部候选与得分。");

    Ok(EXIT_OK)
}

// ---------------------------------------------------------------------------
// info
// ---------------------------------------------------------------------------

/// `pmpx info`：把检测过程整个摊开。
///
/// 像 `plugin current` 一样始终列出全部候选及得分 —— 那是让歧义可见的手段。
fn info(args: &Cli) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    println!("起点       {}", session.start_dir.display());
    match session.project_root() {
        Some(root) => println!("项目根     {}", root.display()),
        None => println!("项目根     （没找到）"),
    }
    println!(
        "上溯       {} 个目录后停止：{}",
        session.walk.dirs.len(),
        session
            .walk
            .stopped
            .describe(session.global.discovery.max_depth)
    );

    if session.project.sources.is_empty() {
        println!("项目配置   （无）");
    } else {
        println!("项目配置   （从近到远，近者优先）");
        for p in &session.project.sources {
            println!("           {}", p.display());
        }
    }
    for (family, plugin) in &session.project.plugin {
        println!("  固化     {family} = \"{plugin}\"");
    }

    println!();

    if session.plugins.plugins.is_empty() {
        println!("已装插件   （无）");
        println!();
        println!("{}", session.no_project_error());
        return Ok(EXIT_OK);
    }

    println!("已装插件");
    for p in &session.plugins.plugins {
        match p.problem() {
            None => println!(
                "  {:<10} {:<8} v{}",
                p.name,
                p.family.as_ref().map(Family::as_str).unwrap_or("?"),
                p.version
            ),
            Some(why) => println!("  {:<10} ⚠ {why}", p.name),
        }
    }
    println!();

    let Some(root) = session.project_root().map(PathBuf::from) else {
        println!("（没有项目根，无法打分）");
        return Ok(EXIT_OK);
    };

    let families = detect::score_all(&session.plugins, &root, &session.project);

    if families.is_empty() {
        println!("候选       （没有可参与裁决的插件）");
        return Ok(EXIT_OK);
    }

    println!("候选与得分");
    let mut rows: Vec<&FamilyScore> = families.values().collect();
    rows.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.family.as_str().cmp(b.family.as_str()))
    });

    for fs in rows {
        let pin = if fs.pinned {
            "  [.pmpx.toml 已固化]"
        } else {
            ""
        };
        println!("  {}  {} 分{pin}", fs.family.display(), fs.score);

        for p in &fs.plugins {
            let hits = p.all_hits().collect::<Vec<_>>().join(", ");
            let detail = if hits.is_empty() {
                "（未命中）".to_string()
            } else {
                hits
            };
            println!("    {:<8} {:>4} 分   {detail}", p.name, p.score);
        }
    }
    println!();

    // 裁决结果
    match session.select(&root) {
        Ok(selection) => {
            session.emit_notes(&selection);
            println!("选中       {}（{}）", selection.name, selection.crate_name);

            match session.load_backend(&selection) {
                Ok(backend) => {
                    let d = backend.diagnostics();
                    println!("  自报名   {}", d.name);
                    println!("  自报生态 {}", d.family);
                    println!("  编译于   {}", d.rustc_version);
                    println!("  target   {}", d.target);

                    if d.family != selection.family.as_str() {
                        println!(
                            "  ⚠ manifest 说它是 {}, 它自己说是 {} —— manifest 被人改过？",
                            selection.family.as_str(),
                            d.family
                        );
                    }
                }
                Err(e) => println!("  ⚠ 加载失败：{e}"),
            }
        }
        Err(failure) => println!("选中       （选不出来）\n{}", failure.message()),
    }

    Ok(EXIT_OK)
}

// ---------------------------------------------------------------------------
// plugin
// ---------------------------------------------------------------------------

fn plugin_cmd(args: &Cli, cmd: &PluginCommand) -> crate::error::Result<u8> {
    match cmd {
        PluginCommand::Ls { flat } => plugin_ls(args, *flat),
        PluginCommand::Current => plugin_current(args),
        PluginCommand::Set { name } => plugin_set(args, name),
        PluginCommand::Unset { family, yes } => plugin_unset(args, family.as_deref(), *yes),
        PluginCommand::Add { names, version } => plugin_add(args, names, version.as_deref()),
        PluginCommand::Rm { names } => plugin_rm(args, names),
        PluginCommand::Update { names, version } => plugin_update(args, names, version.as_deref()),
        PluginCommand::Search { keyword, limit } => plugin_search(args, keyword, *limit),
        PluginCommand::Info { name } => plugin_info(args, name),
    }
}

/// `plugin ls`：按生态分组列出。
fn plugin_ls(args: &Cli, flat: bool) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    if session.plugins.plugins.is_empty() {
        println!("一个插件都没装。");
        println!();
        println!("用 `pmpx plugin add <name>` 装，例如 `pmpx plugin add cargo`。");
        return Ok(EXIT_OK);
    }

    if flat {
        for p in &session.plugins.plugins {
            print_plugin_row(p);
        }
        return Ok(EXIT_OK);
    }

    let mut families: Vec<Option<Family>> = session
        .plugins
        .plugins
        .iter()
        .map(|p| p.family.clone())
        .collect();
    families.sort_by(|a, b| match (a, b) {
        (Some(x), Some(y)) => x.as_str().cmp(y.as_str()),
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, None) => std::cmp::Ordering::Equal,
    });
    families.dedup();

    for family in families {
        let title = family
            .as_ref()
            .map(Family::display)
            .unwrap_or("（未声明生态）");
        println!("{title}");
        for p in session
            .plugins
            .plugins
            .iter()
            .filter(|p| p.family == family)
        {
            print!("  ");
            print_plugin_row(p);
        }
    }

    Ok(EXIT_OK)
}

fn print_plugin_row(p: &crate::plugins::InstalledPlugin) {
    match p.problem() {
        None => println!(
            "{:<10} v{:<10} {:<28} {}",
            p.name,
            p.version,
            p.crate_name,
            p.dir.display()
        ),
        Some(why) => println!("{:<10} ⚠ {why}", p.name),
    }
}

/// `plugin current`：各生态的当前插件，以及候选与得分。
fn plugin_current(args: &Cli) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;
    let Some(root) = session.project_root().map(PathBuf::from) else {
        return Err(session.no_project_error());
    };

    let families = detect::score_all(&session.plugins, &root, &session.project);
    if families.is_empty() {
        println!("没有可参与裁决的插件。");
        return Ok(EXIT_OK);
    }

    let selection = session.select(&root).ok();

    for (family, fs) in &families {
        let current = selection
            .as_ref()
            .filter(|s| &s.family == family)
            .map(|s| s.name.as_str());

        println!("{}", family.display());
        let mut ranked: Vec<&ScoredPlugin> = fs.plugins.iter().collect();
        ranked.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));

        for p in ranked {
            let mark = if Some(p.name.as_str()) == current {
                "←"
            } else {
                " "
            };
            println!("  {} {:<8} {:>4} 分", mark, p.name, p.score);
        }
        if let Some(pinned) = session.project.pinned_plugin(family.as_str()) {
            println!("  固化：{pinned}");
        }
        println!();
    }

    Ok(EXIT_OK)
}

/// `plugin set <name>`：固化到最近的一层 `.pmpx.toml`。
fn plugin_set(args: &Cli, name: &str) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    let Some(plugin) = session.plugins.by_name(name) else {
        return Err(PmpxError::not_found(format!(
            "找不到插件 {name}。已装的是：{}",
            session
                .plugins
                .usable()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    };

    let Some(family) = plugin.family.clone() else {
        return Err(PmpxError::Usage(format!(
            "{name} 的 manifest 没有声明 family，无法固化。"
        )));
    };

    let path = target_config_path(&session);
    edit_project_config(&path, |cfg| {
        cfg.plugin
            .insert(family.as_str().to_string(), plugin.name.clone());
    })
    .map_err(PmpxError::Other)?;

    println!(
        "已固化 {} = \"{}\" 到 {}",
        family.as_str(),
        plugin.name,
        path.display()
    );
    Ok(EXIT_OK)
}

/// `plugin unset [family] [--yes]`
fn plugin_unset(args: &Cli, family: Option<&str>, yes: bool) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;
    let path = target_config_path(&session);

    let existing = ProjectConfig::load_from(&path)
        .map_err(PmpxError::Other)?
        .unwrap_or_default();

    match family {
        Some(f) => {
            if !existing.plugin.contains_key(f) {
                return Err(PmpxError::Usage(format!(
                    "{} 里没有固化过 {f}",
                    path.display()
                )));
            }
            edit_project_config(&path, |cfg| {
                cfg.plugin.remove(f);
            })
            .map_err(PmpxError::Other)?;
            println!("已从 {} 删掉 {f} 的固化", path.display());
        }

        None => {
            if existing.plugin.is_empty() {
                return Err(PmpxError::Usage(format!(
                    "{} 里没有任何固化项",
                    path.display()
                )));
            }

            // 省略 family 且存在多个固化时要求 `--yes`，否则先列出将删除的项并以
            // 退出码 2 退出。
            if existing.plugin.len() > 1 && !yes {
                let mut msg = format!(
                    "{} 里有 {} 个固化项，全部删除需要 `--yes`：",
                    path.display(),
                    existing.plugin.len()
                );
                for (f, p) in &existing.plugin {
                    msg.push_str(&format!("\n  {f} = \"{p}\""));
                }
                return Err(PmpxError::Usage(msg));
            }

            let removed: Vec<String> = existing.plugin.keys().cloned().collect();
            edit_project_config(&path, |cfg| {
                cfg.plugin.clear();
            })
            .map_err(PmpxError::Other)?;
            println!("已从 {} 删掉 {} 的固化", path.display(), removed.join(", "));
        }
    }

    Ok(EXIT_OK)
}

/// 写哪一份 `.pmpx.toml`。
///
/// 最近的一层优先：已经存在的那份就是用户选定的那一层；一份都没有时写项目根。
fn target_config_path(session: &Session) -> PathBuf {
    // `walk.dirs` 是从近到远的
    for dir in &session.walk.dirs {
        let p = dir.join(".pmpx.toml");
        if p.is_file() {
            return p;
        }
    }

    let base = session
        .project_root()
        .map(PathBuf::from)
        .unwrap_or_else(|| session.start_dir.clone());
    base.join(".pmpx.toml")
}

/// 读-改-写一份 `.pmpx.toml`，保留不认识的键。
fn edit_project_config(path: &Path, edit: impl FnOnce(&mut ProjectConfig)) -> Result<()> {
    let mut cfg = ProjectConfig::load_from(path)?.unwrap_or_default();
    edit(&mut cfg);

    // 空配置就别留下一个空文件
    if cfg.plugin.is_empty() && cfg.scripts.is_empty() && cfg.extra.is_empty() {
        if path.is_file() {
            std::fs::remove_file(path)
                .with_context(|| format!("删掉空的配置文件失败：{}", path.display()))?;
        }
        return Ok(());
    }

    let text = toml::to_string_pretty(&cfg).context("序列化项目配置失败")?;
    std::fs::write(path, text).with_context(|| format!("写项目配置失败：{}", path.display()))
}

/// `plugin add`
fn plugin_add(args: &Cli, names: &[String], version: Option<&str>) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    if version.is_some() && names.len() > 1 {
        return Err(PmpxError::Usage(
            "--version 只能与一个插件名同用".to_string(),
        ));
    }

    for name in names {
        println!("正在装 {name}……");
        let installed = session
            .kit
            .install(name, version)
            .map_err(|e| PmpxError::Other(anyhow::anyhow!("{e}")))?;

        println!(
            "  {} v{}（{}）→ {}",
            installed.crate_name,
            installed.version,
            describe_source(installed.source),
            installed.dir.display()
        );
    }

    Ok(EXIT_OK)
}

/// `plugin rm`
fn plugin_rm(args: &Cli, names: &[String]) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    for name in names {
        session
            .kit
            .uninstall(name)
            .map_err(|e| PmpxError::Other(anyhow::anyhow!("{e}")))?;
        println!("已卸掉 {name}");
    }

    Ok(EXIT_OK)
}

/// `plugin update`
fn plugin_update(args: &Cli, names: &[String], version: Option<&str>) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    if version.is_some() && names.len() > 1 {
        return Err(PmpxError::Usage(
            "--version 只能与一个插件名同用".to_string(),
        ));
    }

    // 不带名字 = 全部更新
    let targets: Vec<String> = if names.is_empty() {
        session.plugins.usable().map(|p| p.name.clone()).collect()
    } else {
        names.to_vec()
    };

    if targets.is_empty() {
        println!("没有可更新的插件。");
        return Ok(EXIT_OK);
    }

    for name in targets {
        let installed = session
            .kit
            .update(&name, version)
            .map_err(|e| PmpxError::Other(anyhow::anyhow!("{e}")))?;
        println!("{} → v{}", installed.crate_name, installed.version);
    }

    Ok(EXIT_OK)
}

/// `plugin search` —— 在 crates.io 上搜。
fn plugin_search(args: &Cli, keyword: &str, limit: usize) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    // 用户敲的是短名（`cargo`），而 crates.io 上是 `pmpx-plugin-cargo`。直接搜短名：
    // crates.io 的搜索是全文的，`pmpx-plugin-` 这个前缀不是关键词。
    let results = session
        .kit
        .search(keyword, limit)
        .map_err(|e| PmpxError::Other(anyhow::anyhow!("{e}")))?;

    if results.is_empty() {
        println!("crates.io 上没有匹配 {keyword:?} 的 crate。");
        return Ok(EXIT_OK);
    }

    for r in results {
        println!("{:<32} v{:<10} ↓{}", r.name, r.version, r.downloads);
        if let Some(d) = r.description {
            println!("  {d}");
        }
    }

    Ok(EXIT_OK)
}

/// `plugin info <name>`：本地安装状态 + crates.io 上的信息。
fn plugin_info(args: &Cli, name: &str) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    let mut found = false;

    if let Some(p) = session.plugins.by_name(name) {
        found = true;
        println!("已安装");
        println!("  自报名   {}", p.name);
        println!("  crate    {}", p.crate_name);
        println!("  版本     {}", p.version);
        println!(
            "  生态     {}",
            p.family
                .as_ref()
                .map(Family::as_str)
                .unwrap_or("（未声明）")
        );
        println!(
            "  ABI      {}",
            p.abi
                .map(|a| a.to_string())
                .unwrap_or_else(|| "（未声明）".into())
        );
        println!("  目录     {}", p.dir.display());
        println!("  强证据   {}", join_or_dash(&p.strong));
        println!("  弱证据   {}", join_or_dash(&p.weak));
        if let Some(why) = p.problem() {
            println!("  ⚠ {why}");
        }
        println!();

        if let Ok(backend) = session.load_backend(&crate::detect::Selection {
            crate_name: p.crate_name.clone(),
            name: p.name.clone(),
            family: p.family.clone().unwrap_or_else(|| Family::new("unknown")),
            score: 0,
            notes: Vec::new(),
        }) {
            let d = backend.diagnostics();
            println!("插件自报");
            println!("  名字     {}", d.name);
            println!("  生态     {}", d.family);
            println!("  编译于   {}", d.rustc_version);
            println!("  target   {}", d.target);
            println!();
        }
    }

    // crates.io 那边
    let crate_name = session.kit.config().normalize_crate_name(name);
    match session.kit.view(&crate_name) {
        Ok(Some(info)) => {
            found = true;
            println!("crates.io");
            println!("  crate    {}", info.name);
            println!("  最新     {}", info.version);
            if let Some(d) = info.description {
                println!("  描述     {d}");
            }
            if let Some(r) = info.repository {
                println!("  仓库     {r}");
            }
        }
        Ok(None) => {
            if !found {
                println!("crates.io 上没有 {crate_name}。");
            }
        }
        Err(e) => {
            // 离线 / 网络不通不该让本地信息也白看
            eprintln!("pmpx: 查 crates.io 失败：{e}");
        }
    }

    if !found {
        return Err(PmpxError::not_found(format!("找不到插件 {name}。")));
    }

    Ok(EXIT_OK)
}

fn join_or_dash(items: &[String]) -> String {
    if items.is_empty() {
        "（无）".to_string()
    } else {
        items.join(", ")
    }
}

fn describe_source(source: crate_plugin_kit::cache::InstallSource) -> &'static str {
    match source {
        crate_plugin_kit::cache::InstallSource::Prebuilt => "prebuilt",
        crate_plugin_kit::cache::InstallSource::BuildHost => "本机编译",
    }
}

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

fn config_cmd(cmd: &ConfigCommand) -> crate::error::Result<u8> {
    let path = crate::config::global_config_path().map_err(PmpxError::Other)?;

    match cmd {
        ConfigCommand::Get { key } => {
            let doc = read_toml_table(&path).map_err(PmpxError::Other)?;
            match lookup_dotted(&doc, key) {
                Some(v) => {
                    println!("{}", render_value(v));
                    Ok(EXIT_OK)
                }
                None => Err(PmpxError::Usage(format!(
                    "{} 里没有 {key}（配置文件：{}）",
                    "全局配置",
                    path.display()
                ))),
            }
        }

        ConfigCommand::Set { key, value } => {
            let mut doc = read_toml_table(&path).map_err(PmpxError::Other)?;
            insert_dotted(&mut doc, key, parse_value(value))
                .map_err(|e| PmpxError::Usage(e.to_string()))?;

            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| PmpxError::Other(e.into()))?;
            }
            let text = toml::to_string_pretty(&doc).map_err(|e| PmpxError::Other(e.into()))?;
            std::fs::write(&path, text).map_err(|e| PmpxError::Other(e.into()))?;

            println!(
                "已写入 {key} = {} 到 {}",
                render_value(&parse_value(value)),
                path.display()
            );
            Ok(EXIT_OK)
        }
    }
}

/// 读一份 TOML 成 table；文件不存在 = 空表。
fn read_toml_table(path: &Path) -> Result<toml::Table> {
    match std::fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => {
            toml::from_str(&text).with_context(|| format!("解析 {} 失败", path.display()))
        }
        Ok(_) => Ok(toml::Table::new()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(e) => Err(e).with_context(|| format!("读 {} 失败", path.display())),
    }
}

/// 按 `a.b.c` 取值。
fn lookup_dotted<'a>(table: &'a toml::Table, key: &str) -> Option<&'a toml::Value> {
    let mut parts = key.split('.');
    let first = parts.next()?;
    let mut current = table.get(first)?;

    for part in parts {
        current = current.as_table()?.get(part)?;
    }
    Some(current)
}

/// 按 `a.b.c` 写值，中间缺失的表会被建出来。
fn insert_dotted(table: &mut toml::Table, key: &str, value: toml::Value) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.iter().any(|p| p.is_empty()) {
        anyhow::bail!("键名不能有空段：{key}");
    }

    let mut current = table;
    for part in &parts[..parts.len() - 1] {
        let entry = current
            .entry((*part).to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));

        current = entry
            .as_table_mut()
            .with_context(|| format!("{part} 不是一个表，不能往里写"))?;
    }

    current.insert(parts[parts.len() - 1].to_string(), value);
    Ok(())
}

/// 把命令行上的一串字符解析成 TOML 值。
///
/// 顺序刻意是"最具体到最宽松"：`true` → 整数 → 数组 → 字符串。所以
/// `pmpx config set x 123` 存的是数字，想存字符串就写 `"123"`（带引号）。
fn parse_value(raw: &str) -> toml::Value {
    if raw == "true" {
        return toml::Value::Boolean(true);
    }
    if raw == "false" {
        return toml::Value::Boolean(false);
    }
    if let Ok(n) = raw.parse::<i64>() {
        return toml::Value::Integer(n);
    }

    // 数组 / 带引号的字符串借 TOML 自己的解析器
    if let Ok(doc) = toml::from_str::<toml::Table>(&format!("v = {raw}")) {
        if let Some(v) = doc.get("v") {
            return v.clone();
        }
    }

    toml::Value::String(raw.to_string())
}

/// 打印一个 TOML 值。字符串不加引号 —— 用户要的是值，不是语法。
fn render_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string().trim().to_string(),
    }
}

// ---------------------------------------------------------------------------
// completion
// ---------------------------------------------------------------------------

fn completion(shell: clap_complete::Shell) -> crate::error::Result<u8> {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    // 输出到 stdout，用户自己重定向 —— pmpx 不去猜该往哪个文件写。
    clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_value_recognises_booleans() {
        assert_eq!(parse_value("true"), toml::Value::Boolean(true));
        assert_eq!(parse_value("false"), toml::Value::Boolean(false));
    }

    #[test]
    fn parse_value_recognises_integers() {
        assert_eq!(parse_value("42"), toml::Value::Integer(42));
        assert_eq!(parse_value("-7"), toml::Value::Integer(-7));
    }

    #[test]
    fn parse_value_recognises_arrays() {
        let v = parse_value("[\"rust\", \"node\"]");
        let arr = v.as_array().expect("应当是数组");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str(), Some("rust"));
    }

    #[test]
    fn parse_value_falls_back_to_a_string() {
        assert_eq!(parse_value("node"), toml::Value::String("node".to_string()));
        // 数字想存成字符串就得带引号
        assert_eq!(
            parse_value("\"123\""),
            toml::Value::String("123".to_string())
        );
    }

    #[test]
    fn render_value_omits_quotes_for_strings() {
        assert_eq!(render_value(&toml::Value::String("node".into())), "node");
        assert_eq!(render_value(&toml::Value::Integer(3)), "3");
        assert_eq!(render_value(&toml::Value::Boolean(true)), "true");
    }

    #[test]
    fn lookup_dotted_walks_tables() {
        let doc: toml::Table = toml::from_str(
            r#"
[plugin]
family_priority = ["node"]
[deep]
[deep.er]
x = 1
"#,
        )
        .unwrap();

        assert!(lookup_dotted(&doc, "plugin.family_priority").is_some());
        assert_eq!(
            lookup_dotted(&doc, "deep.er.x"),
            Some(&toml::Value::Integer(1))
        );
        assert!(lookup_dotted(&doc, "plugin.nope").is_none());
        assert!(lookup_dotted(&doc, "nope.at.all").is_none());
    }

    #[test]
    fn insert_dotted_creates_missing_tables() {
        let mut doc = toml::Table::new();
        insert_dotted(&mut doc, "a.b.c", toml::Value::Integer(1)).unwrap();

        assert_eq!(lookup_dotted(&doc, "a.b.c"), Some(&toml::Value::Integer(1)));
    }

    #[test]
    fn insert_dotted_refuses_to_clobber_a_non_table() {
        let mut doc: toml::Table = toml::from_str("a = 1\n").unwrap();
        let err = insert_dotted(&mut doc, "a.b", toml::Value::Integer(2)).unwrap_err();
        assert!(err.to_string().contains("不是一个表"), "{err}");
    }

    #[test]
    fn insert_dotted_rejects_empty_segments() {
        let mut doc = toml::Table::new();
        assert!(insert_dotted(&mut doc, "a..b", toml::Value::Integer(1)).is_err());
        assert!(insert_dotted(&mut doc, "", toml::Value::Integer(1)).is_err());
    }

    #[test]
    fn read_toml_table_treats_a_missing_file_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let t = read_toml_table(&tmp.path().join("nope.toml")).unwrap();
        assert!(t.is_empty());
    }

    #[test]
    fn read_toml_table_treats_an_empty_file_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("empty.toml");
        std::fs::write(&p, "   \n").unwrap();
        assert!(read_toml_table(&p).unwrap().is_empty());
    }

    #[test]
    fn join_or_dash_handles_the_empty_case() {
        assert_eq!(join_or_dash(&[]), "（无）");
        assert_eq!(join_or_dash(&["a".into(), "b".into()]), "a, b");
    }
}
