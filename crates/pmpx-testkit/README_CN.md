# pmpx-testkit

给插件作者的测试助手

插件的 `PackageManager::command` 是纯函数，所以不需要项目、宿主或进程就能测。`pmpx-testkit` 是这件事的三种做法：`context()` 手工构造 `Context`，`Fixture` 写出一个真实目录并在上面跑真实检测，`capture()` 装一个假宿主，让测试可以断言插件写下的行。

## API

- `context() -> pmpx_plugin::ContextBuilder` —— 插件自己的测试所用的构建器。
- `Fixture::new()` —— 在一个真实临时目录上的一个项目。
- `Fixture::file(name, contents)`、`Fixture::plugin(name, family, strong, weak)`、`Fixture::pin(family, plugin)`、`Fixture::prefer(families)`、`Fixture::declares(names)` —— 描述这个项目。
- `Fixture::path()`、`Fixture::context() -> Context<'static>` —— 它在哪，以及宿主会交给插件什么。
- `capture() -> Captured` —— 安装假宿主，并丢掉此前记录的一切。
- `Captured::take()`、`Captured::take_levels()` —— 取空记录下来的行。
- `Captured::level(u32)`、`Captured::trace()`、`Captured::normal()`、`Captured::info()`、`Captured::quiet()` —— 选择要捕获的级别。

## 说明

- 它是给插件作者放进 `[dev-dependencies]` 的；本工作区里没有任何东西依赖它。
- fixture 跑的是真实的 `pmpx_detect::select`，所以插件声明的标记不再匹配时测试会失败。
- `Fixture::declares(...)` 就是白名单：测试写了但没声明的文件永远进不了 context。
- panic 是接口的一部分 —— 本 crate 没有错误类型，什么都没检测到的 fixture 会带着文件清单 panic。

测试：`src/fixture.rs` 与 `src/host.rs` 里的单元测试；没有 `tests/` 目录。
License: 与工作区一致（见仓库根目录）。