//! 一个真的会被 dlopen 的假插件，给 `pmpx` 的端到端测试用 —— 让"检测 → 裁决 →
//! `dlopen` → 跨 ABI 调 `command()` → spawn"整条链路真的跑一遍。
//!
//! 它故意留了几个坑，用来验证宿主的容错：`Exec` 返回 [`PluginError::unsupported_verb`]
//! （宿主必须在 `exec` 上退化成裸透传），`Remove` 直接 panic（宿主必须活着，并拿到
//! `PMPX_ERR_INTERNAL`），其余动词返回一条确实能跑通的命令。

use std::ffi::OsString;

use pmpx_plugin::{CommandSpec, Context, Family, PackageManager, PluginError, Verb};

struct FakePm;

impl PackageManager for FakePm {
    fn name(&self) -> &str {
        // 必须与 manifest 里的 `plugin.name` 一致，否则宿主会拒绝加载。
        "fakepm"
    }

    fn family(&self) -> Family {
        Family::new("faketest")
    }

    fn command(
        &self,
        ctx: &Context,
        verb: Verb,
        args: &[OsString],
    ) -> Result<CommandSpec, PluginError> {
        match verb {
            // 一条一定能跑成功、而且带回显的命令：测试据此断言"跨边界传进来的
            // root / matched / args 都是对的"。
            Verb::Install | Verb::Build | Verb::Test | Verb::Run | Verb::Update => {
                let probe = format!(
                    "pmpx-probe root={} matched={} verb={} args={}",
                    ctx.project_root.display(),
                    ctx.matched.join("|"),
                    verb,
                    args.iter()
                        .map(|a| a.to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join(",")
                );

                // `echo` 在两个平台上都存在，但 Windows 上要用 cmd 内置的那个
                #[cfg(windows)]
                let spec = CommandSpec::new("cmd").arg("/c").arg("echo").arg(probe);
                #[cfg(not(windows))]
                let spec = CommandSpec::new("echo").arg(probe);

                Ok(spec)
            }

            // 明确声明不支持，宿主应当自己裸透传
            Verb::Exec => Err(PluginError::unsupported_verb(verb)),

            // 验证 panic 兜底。绝不能穿过 `extern "C"`。
            Verb::Remove => panic!("假插件在 remove 上故意炸一次"),

            // Verb 是封闭枚举，这里理论上到不了；留着是为了将来加动词时不会静默放过
            #[allow(unreachable_patterns)]
            other => Err(PluginError::other(format!("{other} 没实现"))),
        }
    }
}

/// 工厂函数。crate-plugin-kit 生成的 wrapper 调的也是它。
pub fn create() -> Box<dyn PackageManager> {
    Box::new(FakePm)
}

pmpx_plugin::export!(create);
