//! Why no plugin could be selected.
//!
//! Separate from the selection itself ([`super::resolve`]) because each variant exists to become
//! one actionable sentence: "nothing is installed" and "the pinned plugin is installed but broken"
//! have completely different next steps, so they must not be reported as the same thing.

/// Why no selection could be made.
///
/// Every variant corresponds to one sentence that **tells the user what to do next**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectFailure {
    /// Every candidate scored 0 and nothing is pinned.
    NothingDetected,
    /// `.pmpx.toml` pins a plugin that is not installed.
    PinnedNotInstalled {
        /// Family name.
        family: String,
        /// The pinned plugin name.
        name: String,
    },
    /// The plugin pinned in `.pmpx.toml` **is** installed, but cannot take part in resolution.
    ///
    /// Kept separate from [`DetectFailure::PinnedNotInstalled`] because the next step is completely
    /// different: "not installed" means go install it, "installed but broken" means look at the
    /// reason shown in `pmpx plugin ls`.
    PluginUnusable {
        /// Plugin name.
        name: String,
        /// Why it cannot be used.
        problem: String,
    },
    /// The plugin named by `-p` does not exist (or cannot take part in resolution).
    UnknownPlugin {
        /// The name the user gave.
        name: String,
        /// The available choices, already sorted.
        available: Vec<String>,
    },
    /// The winning family has no usable plugin at all.
    ///
    /// **Defensive branch**: unreachable on the normal path — a family score either comes from a
    /// plugin (then it has one) or from a pin (then it is caught by the two variants above first).
    /// A useful sentence beats `unreachable!()`.
    FamilyWithoutPlugin {
        /// Family name.
        family: String,
        /// That family's score.
        score: u32,
    },
}

impl DetectFailure {
    /// The full explanation shown to the user; it must include what to do next.
    pub fn message(&self) -> String {
        match self {
            DetectFailure::NothingDetected => "Cannot detect the project type.\n\
                 pmpx decides the project type from the detect files declared by installed plugins, so:\n\
                   - with no plugin installed, no project can be detected (`pmpx plugin add <name>`)\n\
                   - you can also declare it explicitly with a .pmpx.toml in the current directory, for example:\n\
                     [plugin]\n\
                     rust = \"cargo\""
                .to_string(),
            DetectFailure::FamilyWithoutPlugin { family, score } => format!(
                "The winning family is {family} ({score} points), but no plugin belonging to it is installed.\n\
                 pmpx only knows which tool to invoke once one is installed."
            ),
            DetectFailure::PinnedNotInstalled { family, name } => format!(
                ".pmpx.toml pins {family} to {name}, but it is not installed.\n\
                 Either install it (`pmpx plugin add {name}`) or change that pin."
            ),
            DetectFailure::PluginUnusable { name, problem } => format!(
                "{name} is installed but cannot take part in resolution: {problem}.\n\
                 Use `pmpx plugin ls` to see every plugin and its own problem."
            ),
            DetectFailure::UnknownPlugin { name, available } => {
                let mut msg = format!("Plugin {name} not found.");
                if available.is_empty() {
                    msg.push_str("\nNo plugin is installed at all (`pmpx plugin add <name>`).");
                } else {
                    msg.push_str("\nInstalled: ");
                    msg.push_str(&available.join(", "));
                }
                msg
            }
        }
    }
}

/// Nothing selected = **exit code 3**: pmpx itself is fine, the environment is missing something.
/// User scripts should be able to use this code to tell "pmpx is broken" (1) from "you have not
/// installed anything yet" (3).
impl From<DetectFailure> for crate::error::PmpxError {
    fn from(f: DetectFailure) -> Self {
        crate::error::PmpxError::NotFound(f.message())
    }
}
