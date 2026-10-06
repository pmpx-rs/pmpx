# pmpx-detect

Decides which installed plugin a project belongs to

`pmpx-detect` is the decision layer of pmpx. It takes the installed plugins as `Candidate` values, the
project's files through the single-method `Presence` trait, and the pins and orderings as plain values.
It returns a `Selection` or a typed `DetectFailure`. It has no dependencies: it reads no file, prints
nothing and knows no ecosystem.

## API

- `Presence::has(&self, relative: &str) -> bool` — the only filesystem-shaped thing here; implemented
  for `BTreeSet<String>` and for `Fn(&str) -> bool`.
- `Candidate { crate_name, name, family, strong, weak, problem }` and `Candidate::is_usable()`.
- `Pins = BTreeMap<String, String>` (family → plugin name) and
  `Preferences { family_priority, priority }`.
- `ScoredPlugin::score(plugin, presence)` — fields `strong_hits`, `weak_hits`, `score`, plus `all_hits()`.
- `score_all(candidates, presence, pins) -> BTreeMap<String, FamilyScore>`.
- `select(candidates, presence, pins, preferences, explicit: Option<&str>) -> Result<Selection, DetectFailure>`
  and `select_from_scores(...)` when the caller already scored.
- `Selection { crate_name, name, family, score, reason, matched, notes }`,
  `Reason::{Scored, Pinned, Explicit}`, `DetectFailure` (five variants, plus `message()`).
- Constants `STRONG_SCORE = 100`, `WEAK_SCORE = 10`, `PIN_FLOOR = 50`.

## Notes

- The decision eats data, not the filesystem: the caller supplies all evidence as `Candidate::strong` /
  `Candidate::weak` marker names, and `Presence` answers "is this file here".
- Scoring is 100 per matched strong marker plus 10 per matched weak one; a family takes the highest
  score among its plugins, never the sum.
- A pin floors a family at `PIN_FLOOR` (50) instead of overwriting its score, so it beats weak evidence
  but never a real lockfile.
- `-p <name>` overrides both pins and evidence; `Selection::matched` is the evidence that picked the
  winner; a tie is reported in `notes` but still returns a winner.

Tests: unit tests in `src/lib.rs` and `src/failure.rs`, plus `tests/no_filesystem.rs`; all in memory.
License: same as the workspace (see the repository root).