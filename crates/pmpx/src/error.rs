//! 错误类型与退出码。
//!
//! 退出码是用户脚本能依赖的接口（`pmpx test || echo failed` 必须能工作），所以它
//! 是错误类型的一部分，而不是在 `main` 里靠匹配字符串猜出来的。
//!
//! 唯一的例外是后端进程的退出码原样透传：那不是错误路径，是正常流程，走 `Ok(code)`。

/// pmpx 的成功/失败结果。
pub type Result<T> = std::result::Result<T, PmpxError>;

/// 退出码 0：成功。
pub const EXIT_OK: u8 = 0;
/// 退出码 1：pmpx 自身错误（配置解析失败、文件锁超时、插件删除冲突等）。
pub const EXIT_INTERNAL: u8 = 1;
/// 退出码 2：用法错误 / 不支持的动词。
pub const EXIT_USAGE: u8 = 2;
/// 退出码 3：检测不到项目类型 / 缺少能处理该生态的插件 / 后端可执行文件找不到。
pub const EXIT_NOT_FOUND: u8 = 3;

/// pmpx 的错误。
///
/// 每个变体对应一类由 pmpx 自己产生的退出码；后端退出码不是错误，见文件头。
#[derive(Debug, thiserror::Error)]
pub enum PmpxError {
    /// 用法错误，或者插件明确不支持这个动词。
    ///
    /// 「不支持」也归这里：用户敲的动词是合法的，是这个后端做不到。
    #[error("{0}")]
    Usage(String),

    /// 检测不到项目类型 / 缺少能处理该生态的插件 / 后端可执行文件找不到。
    ///
    /// 共同点是 pmpx 本身没问题，是环境里缺东西；消息里带着"下一步该做什么"。
    #[error("{0}")]
    NotFound(String),

    /// pmpx 自身出错了。这里是兜底，正常情况下不该走到。
    #[error(transparent)]
    Other(#[from] anyhow::Error),

    /// **后端**报出来的失败，退出码由它决定，不是 pmpx 自己的码。
    ///
    /// 不能折进 [`PmpxError::Usage`]：那样"插件不支持这个动词"和"插件自己炸了"
    /// 就没法区分，而脚本需要分开处理。
    #[error("{0}")]
    Backend(String, u8),
}

impl PmpxError {
    /// 这条错误该让进程以什么码退出。
    pub fn exit_code(&self) -> u8 {
        match self {
            PmpxError::Usage(_) => EXIT_USAGE,
            PmpxError::NotFound(_) => EXIT_NOT_FOUND,
            PmpxError::Other(_) => EXIT_INTERNAL,
            PmpxError::Backend(_, code) => *code,
        }
    }

    /// 构造一条"环境里缺东西"。
    pub fn not_found(message: impl Into<String>) -> Self {
        PmpxError::NotFound(message.into())
    }
}
