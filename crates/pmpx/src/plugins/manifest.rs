//! One manifest into one [`InstalledPlugin`], including the `[detect]` section that
//! `crate-plugin-kit` does not know about.

use anyhow::Result;
use crate_plugin_kit::{CratePluginKit, PluginInfo, PluginManifest};
use pmpx_plugin::abi::PmpxPluginV1;
use pmpx_plugin::Family;

use super::InstalledPlugin;

/// Complete a [`PluginInfo`] into an [`InstalledPlugin`] (reading the manifest again for `[detect]`).
pub(super) fn read_one(
    kit: &CratePluginKit<PmpxPluginV1>,
    info: &PluginInfo,
) -> Result<InstalledPlugin> {
    // `list()` just read this manifest successfully, so a failure here can only mean the file
    // disappeared between the two reads. Fall back to an empty detect then — the plugin is still
    // listed, it just does not take part in detection.
    let manifest = kit.manifest_of(&info.crate_name).ok();
    let (strong, weak) = manifest.as_ref().map(detect_patterns).unwrap_or_default();

    Ok(InstalledPlugin {
        name: info.name.clone(),
        crate_name: info.crate_name.clone(),
        version: info.version.clone(),
        family: info.family.clone().map(Family::new),
        abi: info.abi,
        dir: info.dir.clone(),
        strong,
        weak,
    })
}

/// Dig `[detect]` out of the manifest's unrecognized fields.
///
/// `crate-plugin-kit` does not know it, so it is left as-is in `extra`.
fn detect_patterns(manifest: &PluginManifest) -> (Vec<String>, Vec<String>) {
    let Some(detect) = manifest.extra.get("detect") else {
        return (Vec::new(), Vec::new());
    };

    (
        str_array(detect.get("strong")),
        str_array(detect.get("weak")),
    )
}

/// Read a TOML value as an array of strings; when it is not an array, or an element is not a
/// string, skip it instead of erroring — a malformed `detect` section must not make the whole
/// plugin disappear from the manifest.
fn str_array(value: Option<&toml::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
