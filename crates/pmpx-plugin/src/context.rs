//! What the host hands to a plugin: everything it knows about this call.
//!
//! Two kinds of thing live here, and the split is deliberate:
//!
//! - **What the project is** -- the root, the invocation directory, the matched files, the project
//!   config that was read, and the contents of the files the plugin declared it wants to see.
//! - **What the host decided** -- why this plugin was selected, with what score, and what the project
//!   config pins.
//!
//! The cheap parts are copied out of the host once per call; **file contents are not**. A plugin asks
//! for `file.<name>` when it wants it, so a manifest that declares a dozen files costs nothing until
//! one of them is actually needed -- and a file the manifest did not declare is never available at
//! all, which is what keeps the declaration the allowlist.

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::path::PathBuf;

/// Why the host selected this plugin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SelectionReason {
    /// It won on evidence: the highest score in the winning family.
    ///
    /// Also the default: a hand-built context (in a plugin's own test) starts from "nothing was
    /// pinned and nobody named me", which is this.
    #[default]
    Scored,
    /// The project config pins this plugin's family to it.
    Pinned,
    /// The user named it on the command line.
    Explicit,
    /// A reason this build of the contract does not know.
    ///
    /// A newer host may add one, and an older plugin must still run: the reason is information, not
    /// something to refuse a call over.
    Unknown,
}

impl SelectionReason {
    /// Read the number the host sent.
    pub const fn from_abi(raw: u32) -> Self {
        match raw {
            pmpx_plugin_abi::PMPX_REASON_SCORED => Self::Scored,
            pmpx_plugin_abi::PMPX_REASON_PINNED => Self::Pinned,
            pmpx_plugin_abi::PMPX_REASON_EXPLICIT => Self::Explicit,
            _ => Self::Unknown,
        }
    }

    /// The word this reason is called by, for a plugin's own messages.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scored => "scored",
            Self::Pinned => "pinned",
            Self::Explicit => "explicit",
            Self::Unknown => "unknown",
        }
    }

    /// The number this reason crosses the boundary as.
    ///
    /// [`SelectionReason::Unknown`] has none of its own -- it means "this build does not know that
    /// reason" and only ever comes *from* a host -- so it answers with the neutral
    /// [`PMPX_REASON_SCORED`](pmpx_plugin_abi::PMPX_REASON_SCORED).
    pub const fn to_abi(self) -> u32 {
        match self {
            Self::Scored | Self::Unknown => pmpx_plugin_abi::PMPX_REASON_SCORED,
            Self::Pinned => pmpx_plugin_abi::PMPX_REASON_PINNED,
            Self::Explicit => pmpx_plugin_abi::PMPX_REASON_EXPLICIT,
        }
    }
}

impl std::fmt::Display for SelectionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The contents of one file a plugin asked to see.
///
/// Owned rather than borrowed, because a plugin can hold onto it: asking for a file twice asks the
/// host twice, which is cheap for the host (it caches) and means no lifetime has to be threaded
/// through a plugin's own code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFile {
    /// The path as the plugin's manifest declared it.
    pub name: String,
    /// The bytes, which need not be UTF-8: how to read them is the plugin's business.
    pub bytes: Vec<u8>,
}

impl ContextFile {
    /// The contents as text, or `None` when they are not UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }
}

/// Where a context's file contents come from.
#[derive(Debug, Clone)]
enum Files<'a> {
    /// The host answers on demand, through the ABI's accessors. Nothing is read until it is asked
    /// for, and only names the manifest declared can be answered at all.
    Host {
        context: *const pmpx_plugin_abi::PmpxContext,
        /// Ties the raw pointer to the call it belongs to, so a `Context` cannot outlive it.
        marker: PhantomData<&'a ()>,
    },
    /// A table a plugin's own test supplied.
    Table(BTreeMap<String, Vec<u8>>),
}

/// The context the host passes to the plugin. Read-only.
#[derive(Debug, Clone)]
pub struct Context<'a> {
    /// Project root: where the command runs unless the answer names a `cwd` of its own.
    pub project_root: PathBuf,

    /// The directory the person ran pmpx from (its `-C`, or the process's own directory).
    ///
    /// It differs from `project_root` whenever the root was found by walking up, which makes this
    /// the only way to tell which package of a monorepo the command is for. **Not** where the
    /// command will run: that is the `cwd` of the answer, and it defaults to `project_root`.
    ///
    /// Empty when the host had nothing to say about it.
    pub start_dir: PathBuf,

    /// The files this detection matched, relative to `project_root`.
    ///
    /// This is the plugin's main channel for learning "what the project looks like". Example: the
    /// yarn plugin distinguishes classic from berry via `has_matched(".yarnrc.yml")` without
    /// reading a single file.
    pub matched: Vec<String>,

    /// The project config files that were read, nearest first.
    pub config_files: Vec<PathBuf>,

    /// `[plugin]` pins from the project config: family → plugin name.
    ///
    /// The whole map, not just this plugin's family: it is how a plugin can notice that the project
    /// pins something for its family which is *not* installed, and say so.
    pub pins: BTreeMap<String, String>,

    /// Why this plugin was selected.
    pub reason: SelectionReason,

    /// The evidence score it won with. 0 when it was pinned or named outright, since neither needed
    /// evidence.
    pub score: u32,

    files: Files<'a>,
}

impl Default for Context<'_> {
    fn default() -> Self {
        Self {
            project_root: PathBuf::new(),
            start_dir: PathBuf::new(),
            matched: Vec::new(),
            config_files: Vec::new(),
            pins: BTreeMap::new(),
            reason: SelectionReason::Scored,
            score: 0,
            files: Files::Table(BTreeMap::new()),
        }
    }
}

impl<'a> Context<'a> {
    /// A context for a plugin's own test: hand-built, with no host behind it.
    ///
    /// ```ignore
    /// let ctx = Context::builder()
    ///     .project_root("/work")
    ///     .matched(["pnpm-lock.yaml"])
    ///     .file("package.json", "{\"name\":\"x\"}")
    ///     .build();
    /// ```
    pub fn builder() -> ContextBuilder {
        ContextBuilder {
            context: Context::default(),
        }
    }

    /// Build the context a call describes, reading the cheap parts now and leaving file contents to
    /// be asked for.
    ///
    /// # Safety
    /// `context` must be a context the host built, valid for the whole call.
    pub(crate) unsafe fn from_host(context: *const pmpx_plugin_abi::PmpxContext) -> Context<'a> {
        // SAFETY: the caller vouches for the pointer; every accessor below is called with it.
        let raw = unsafe { &*context };

        Context {
            // SAFETY: as above. Paths are read as raw bytes and converted per platform.
            project_root: PathBuf::from(pmpx_plugin_abi::bytes_to_os(
                &unsafe { read_key(context, pmpx_plugin_abi::PMPX_KEY_PROJECT_ROOT, 0) }
                    .unwrap_or_default(),
            )),
            start_dir: PathBuf::from(pmpx_plugin_abi::bytes_to_os(
                &unsafe { read_key(context, pmpx_plugin_abi::PMPX_KEY_PROJECT_START_DIR, 0) }
                    .unwrap_or_default(),
            )),
            // SAFETY: as above.
            matched: unsafe { read_list(context, pmpx_plugin_abi::PMPX_KEY_PROJECT_MATCHED) }
                .into_iter()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .collect(),
            config_files: unsafe {
                read_list(context, pmpx_plugin_abi::PMPX_KEY_PROJECT_CONFIG_FILES)
            }
            .into_iter()
            .map(|bytes| PathBuf::from(pmpx_plugin_abi::bytes_to_os(&bytes)))
            .collect(),
            pins: {
                // Names and values: `config.pin` is the one map-shaped key.
                let count = unsafe { key_count(raw, pmpx_plugin_abi::PMPX_KEY_CONFIG_PIN) };
                let mut pins = BTreeMap::new();
                for index in 0..count {
                    let family =
                        unsafe { read_name(context, pmpx_plugin_abi::PMPX_KEY_CONFIG_PIN, index) };
                    let plugin =
                        unsafe { read_key(context, pmpx_plugin_abi::PMPX_KEY_CONFIG_PIN, index) };
                    if let (Some(family), Some(plugin)) = (family, plugin) {
                        pins.insert(
                            String::from_utf8_lossy(&family).into_owned(),
                            String::from_utf8_lossy(&plugin).into_owned(),
                        );
                    }
                }
                pins
            },
            reason: SelectionReason::from_abi(raw.reason),
            score: raw.score,
            files: Files::Host {
                context,
                marker: PhantomData,
            },
        }
    }

    /// Whether one of the matched files is this one -- the standard way for a plugin to branch on
    /// shape.
    pub fn has_matched(&self, file: &str) -> bool {
        self.matched.iter().any(|m| m == file)
    }

    /// Whether the project config is why this plugin is the one being asked.
    pub fn was_pinned(&self) -> bool {
        self.reason == SelectionReason::Pinned
    }

    /// What the project pins this plugin's `family` to, if anything.
    pub fn pinned_for(&self, family: &str) -> Option<&str> {
        self.pins.get(family).map(String::as_str)
    }

    /// The contents of one file this plugin declared in its manifest's `[context] files`.
    ///
    /// The host reads it now if it has not been read yet. `None` means it could not be handed over --
    /// a missing file, one that cannot be read, or a name the manifest did not declare. Whether that
    /// matters is the plugin's call: a missing lockfile and a missing optional config are different
    /// things.
    pub fn file(&self, name: &str) -> Option<ContextFile> {
        let bytes = match &self.files {
            Files::Table(table) => table.get(name).cloned(),
            // SAFETY: the context is valid for as long as this `Context` -- that is what the
            // lifetime parameter on the variant is for.
            Files::Host { context, .. } => {
                let key = format!("{}{name}", pmpx_plugin_abi::PMPX_KEY_FILE_PREFIX);
                unsafe { read_key(*context, &key, 0) }
            }
        }?;

        Some(ContextFile {
            name: name.to_string(),
            bytes,
        })
    }

    /// The same, as text: `None` when the file was not handed over or is not UTF-8.
    pub fn file_str(&self, name: &str) -> Option<String> {
        let file = self.file(name)?;
        file.as_str().map(str::to_string)
    }

    /// One line describing this context, for a plugin's own logging.
    pub(crate) fn describe(&self, verb: crate::Verb, args_len: usize) -> String {
        let pins: Vec<String> = self
            .pins
            .iter()
            .map(|(family, plugin)| format!("{family}={plugin}"))
            .collect();
        let configs: Vec<String> = self
            .config_files
            .iter()
            .map(|path| path.display().to_string())
            .collect();

        format!(
            "context: root={} start={} matched=[{}] verb={} args={} reason={} score={} pins=[{}] config=[{}]",
            self.project_root.display(),
            self.start_dir.display(),
            self.matched.join(" "),
            verb,
            args_len,
            self.reason,
            self.score,
            pins.join(" "),
            configs.join(" "),
        )
    }
}

/// Assemble a context by hand, for a plugin's own tests.
pub struct ContextBuilder {
    context: Context<'static>,
}

impl ContextBuilder {
    /// Set the project root.
    pub fn project_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.context.project_root = root.into();
        self
    }

    /// Set the invocation directory.
    pub fn start_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.context.start_dir = dir.into();
        self
    }

    /// Set the matched files.
    pub fn matched<I, S>(mut self, files: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.context.matched = files.into_iter().map(Into::into).collect();
        self
    }

    /// Set the project config files that would have been read.
    pub fn config_files<I, P>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.context.config_files = paths.into_iter().map(Into::into).collect();
        self
    }

    /// Pin one family to one plugin.
    pub fn pin(mut self, family: &str, plugin: &str) -> Self {
        self.context
            .pins
            .insert(family.to_string(), plugin.to_string());
        self
    }

    /// Say why this plugin would have been selected.
    pub fn reason(mut self, reason: SelectionReason) -> Self {
        self.context.reason = reason;
        self
    }

    /// Set the evidence score.
    pub fn score(mut self, score: u32) -> Self {
        self.context.score = score;
        self
    }

    /// Offer the contents of one declared file.
    pub fn file(mut self, name: &str, contents: impl Into<Vec<u8>>) -> Self {
        if let Files::Table(table) = &mut self.context.files {
            table.insert(name.to_string(), contents.into());
        }
        self
    }

    /// Build it.
    pub fn build(self) -> Context<'static> {
        self.context
    }
}

/// How many values one key has, through the host's accessor.
///
/// # Safety
/// `context` must be a valid host context.
unsafe fn key_count(context: &pmpx_plugin_abi::PmpxContext, key: &str) -> usize {
    let count = context.count;
    let key = pmpx_plugin_abi::PmpxStr::new(key.as_ptr(), key.len());
    // SAFETY: the caller vouches for the context, and the key borrows for the length of the call.
    unsafe { count(context, key) }
}

/// Read one value, or `None` when the host has nothing under that key.
///
/// # Safety
/// As [`key_count`].
unsafe fn read_key(
    context: *const pmpx_plugin_abi::PmpxContext,
    key: &str,
    index: usize,
) -> Option<Vec<u8>> {
    // SAFETY: the caller vouches for the context.
    let raw = unsafe { &*context };
    if unsafe { key_count(raw, key) } <= index {
        return None;
    }

    let get = raw.get;
    let key = pmpx_plugin_abi::PmpxStr::new(key.as_ptr(), key.len());
    // SAFETY: as above.
    let value = unsafe { get(context, key, index) };
    // SAFETY: the host keeps this valid for the call.
    unsafe { value.as_bytes() }.map(<[u8]>::to_vec)
}

/// Read every value of one list-shaped key.
///
/// # Safety
/// As [`key_count`].
unsafe fn read_list(context: *const pmpx_plugin_abi::PmpxContext, key: &str) -> Vec<Vec<u8>> {
    // SAFETY: the caller vouches for the context.
    let raw = unsafe { &*context };
    let count = unsafe { key_count(raw, key) };

    // A host that answers an absurd count is refused rather than allocated for: this is the same
    // ceiling the ABI puts on every array.
    if count > pmpx_plugin_abi::PMPX_MAX_ITEMS {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        if let Some(bytes) = unsafe { read_key(context, key, index) } {
            out.push(bytes);
        }
    }
    out
}

/// Read one *name* of a map-shaped key.
///
/// # Safety
/// As [`key_count`].
unsafe fn read_name(
    context: *const pmpx_plugin_abi::PmpxContext,
    key: &str,
    index: usize,
) -> Option<Vec<u8>> {
    // SAFETY: the caller vouches for the context.
    let raw = unsafe { &*context };
    if unsafe { key_count(raw, key) } <= index {
        return None;
    }

    let name = raw.name;
    let key = pmpx_plugin_abi::PmpxStr::new(key.as_ptr(), key.len());
    // SAFETY: as above.
    let value = unsafe { name(context, key, index) };
    // SAFETY: the host keeps this valid for the call.
    unsafe { value.as_bytes() }.map(<[u8]>::to_vec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hand_built_context_answers_like_a_host_would() {
        let context = Context::builder()
            .project_root("/work/project")
            .start_dir("/work/project/packages/api")
            .matched(["package.json", "pnpm-lock.yaml"])
            .config_files(["/work/project/.pmpx.toml"])
            .pin("node", "pnpm")
            .reason(SelectionReason::Pinned)
            .score(110)
            .file("package.json", "{\"name\":\"x\"}")
            .build();

        assert_eq!(context.project_root, PathBuf::from("/work/project"));
        assert_eq!(
            context.start_dir,
            PathBuf::from("/work/project/packages/api"),
            "the invocation directory is not the root"
        );
        assert!(context.has_matched("package.json"));
        assert!(!context.has_matched("Cargo.toml"));
        assert!(context.was_pinned());
        assert_eq!(context.pinned_for("node"), Some("pnpm"));
        assert_eq!(context.pinned_for("rust"), None);
        assert_eq!(context.score, 110);
    }

    /// A declared file is answered; an undeclared one is not, and neither is a missing one.
    #[test]
    fn only_declared_files_are_answered() {
        let context = Context::builder()
            .file("package.json", "{\"name\":\"x\"}")
            .build();

        assert_eq!(
            context.file_str("package.json").as_deref(),
            Some("{\"name\":\"x\"}")
        );
        assert_eq!(
            context.file("package.json").map(|f| f.bytes),
            Some(b"{\"name\":\"x\"}".to_vec())
        );
        assert!(context.file("Cargo.toml").is_none(), "not declared");
        assert!(context.file_str("package.json").is_some());
    }

    #[test]
    fn a_file_that_is_not_utf8_has_no_text() {
        let context = Context::builder().file("binary", [0xffu8, 0xfe]).build();

        assert!(context.file("binary").is_some(), "the bytes are there");
        assert!(
            context.file_str("binary").is_none(),
            "but they are not text"
        );
    }

    #[test]
    fn the_default_context_knows_nothing() {
        let context = Context::default();

        assert!(context.project_root.as_os_str().is_empty());
        assert!(context.matched.is_empty());
        assert!(context.pins.is_empty());
        assert!(context.file("anything").is_none());
        assert_eq!(context.reason, SelectionReason::Scored);
        assert_eq!(context.score, 0);
    }

    #[test]
    fn the_reason_round_trips_and_has_a_word() {
        for (reason, number) in [
            (SelectionReason::Scored, pmpx_plugin_abi::PMPX_REASON_SCORED),
            (SelectionReason::Pinned, pmpx_plugin_abi::PMPX_REASON_PINNED),
            (
                SelectionReason::Explicit,
                pmpx_plugin_abi::PMPX_REASON_EXPLICIT,
            ),
        ] {
            assert_eq!(SelectionReason::from_abi(number), reason);
            assert_eq!(reason.to_abi(), number);
            assert!(!reason.as_str().is_empty());
        }

        // A reason this build does not know is information, not a refusal.
        assert_eq!(SelectionReason::from_abi(999), SelectionReason::Unknown);
        assert_eq!(
            SelectionReason::Unknown.to_abi(),
            pmpx_plugin_abi::PMPX_REASON_SCORED
        );
    }

    #[test]
    fn the_description_names_what_the_host_said() {
        let context = Context::builder()
            .project_root("/work/project")
            .start_dir("/work/packages/api")
            .matched(["package.json"])
            .pin("node", "pnpm")
            .config_files(["/work/.pmpx.toml"])
            .reason(SelectionReason::Pinned)
            .score(110)
            .build();

        let line = context.describe(crate::Verb::Install, 2);

        assert!(line.contains("/work/project"), "{line}");
        assert!(line.contains("start=/work/packages/api"), "{line}");
        assert!(line.contains("package.json"), "{line}");
        assert!(line.contains("verb=install"), "{line}");
        assert!(line.contains("args=2"), "{line}");
        assert!(line.contains("reason=pinned"), "{line}");
        assert!(line.contains("score=110"), "{line}");
        assert!(line.contains("node=pnpm"), "{line}");
        assert!(line.contains(".pmpx.toml"), "{line}");
    }
}
