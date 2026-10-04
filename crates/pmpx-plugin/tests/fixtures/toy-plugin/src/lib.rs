//! 一个真的会被编成 cdylib 的假插件。
//!
//! 它存在的意义只有一个：证明 `export!` 生成的外壳**确实能变成一个可加载的动态库**，
//! 而不只是在同一个进程里自己调自己。它零依赖（除了被它测试的 `pmpx-plugin`）。

use std::ffi::OsString;

use pmpx_plugin::{CommandSpec, Context, Family, PackageManager, PluginError, Verb};

struct Toy;

impl PackageManager for Toy {
    fn name(&self) -> &str {
        "toy"
    }

    fn family(&self) -> Family {
        Family::NODE
    }

    fn command(
        &self,
        ctx: &Context,
        verb: Verb,
        args: &[OsString],
    ) -> Result<CommandSpec, PluginError> {
        match verb {
            // 把命中列表回显成一个参数，宿主就能**跨 dlopen 边界**验证 `Context::matched`
            // 真的传进来了。
            Verb::Install => {
                let mut spec = CommandSpec::new("toy-bin").arg("add").args(args.iter());
                if ctx.has_matched(".yarnrc.yml") {
                    spec = spec.arg("--berry");
                }
                Ok(spec)
            }

            // 加上项目根，验证路径也跨过来了
            Verb::Run => Ok(CommandSpec::new("toy-bin").arg(ctx.project_root.clone())),

            // 明确不支持，让宿主去降级
            Verb::Exec => Err(PluginError::unsupported_verb(verb)),

            // 故意 panic，验证 guard 与 `panic = "unwind"`
            Verb::Test => panic!("这个 panic 必须被 guard 抓住，绝不能穿过 extern \"C\""),

            other => Err(PluginError::other(format!("{other} 还没实现"))),
        }
    }
}

/// 工厂函数。crate-plugin-kit 生成的 wrapper 工程里调的就是它。
pub fn create() -> Box<dyn PackageManager> {
    Box::new(Toy)
}

pmpx_plugin::export!(create);
