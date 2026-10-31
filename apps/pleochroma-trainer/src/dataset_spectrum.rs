//! The trainer samples the renderer's complete built-in optical material catalogue.
//!
//! No named species is replaced by a neutral index-only surrogate.

use super::DatasetError;
use pleochroma::optics::materials::GemMaterial;
use pleochroma_ai::model::Prng;
use pleochroma_cut_core::ResolvedMaterial;
use std::sync::LazyLock;

/// A catalogue preset and its sodium-D optical summary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MineralSpectrumEntry {
    /// Exact preset name, including any variety suffix.
    pub name: &'static str,
    /// Always present; retained for the GUI's catalogue interface.
    pub preset: Option<&'static str>,
    /// Refractive index at the sodium D line.
    pub refractive_index: f32,
    /// Critical angle in degrees.
    pub critical_angle_deg: f32,
}

/// All real built-ins, sorted by index then name for deterministic sampling and display.
pub static MINERAL_SPECTRUM: LazyLock<Vec<MineralSpectrumEntry>> = LazyLock::new(|| {
    let mut entries: Vec<_> = GemMaterial::built_in()
        .iter()
        .map(|gem| {
            let refractive_index = gem.dispersion.evaluate(589.3);
            MineralSpectrumEntry {
                name: gem.name.as_str(),
                preset: Some(gem.name.as_str()),
                refractive_index,
                critical_angle_deg: (1.0 / refractive_index).asin().to_degrees(),
            }
        })
        .collect();
    entries.sort_by(|a, b| {
        a.refractive_index
            .total_cmp(&b.refractive_index)
            .then_with(|| a.name.cmp(b.name))
    });
    entries
});

/// Resolve an exact name or a documented species alias.
///
/// # Errors
/// Unknown names are rejected rather than replaced by neutral or diamond surrogates.
pub fn find_spectrum_entry(name: &str) -> Result<MineralSpectrumEntry, DatasetError> {
    // Only strip the GUI numeric suffix: parentheses in actual variety names matter.
    let name = name.split(" (n=").next().unwrap_or(name).trim();
    let name = match name.to_ascii_lowercase().as_str() {
        "corundum" => "Sapphire",
        "moissanite" => "Synthetic Moissanite",
        "cz" => "Cubic Zirconia",
        _ => name,
    };
    MINERAL_SPECTRUM
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(name))
        .copied()
        .ok_or_else(|| DatasetError::UnknownMaterial(name.to_owned()))
}

/// Resolve a genuine catalogue entry with its full optical properties.
///
/// # Errors
/// Returns `UnknownMaterial` for an entry not naming an exact built-in.
pub fn resolve_entry(entry: &MineralSpectrumEntry) -> Result<ResolvedMaterial, DatasetError> {
    if entry.preset != Some(entry.name) {
        return Err(DatasetError::UnknownMaterial(entry.name.to_owned()));
    }
    let exact = find_spectrum_entry(entry.name)?;
    let gem = GemMaterial::built_in()
        .iter()
        .find(|gem| gem.name == exact.name)
        .ok_or_else(|| DatasetError::UnknownMaterial(exact.name.to_owned()))?
        .clone();
    let n_d = f64::from(gem.dispersion.evaluate(589.3));
    Ok(ResolvedMaterial {
        gem,
        n_d,
        critical_angle_deg: pleochroma_cut_core::critical_angle_deg(n_d),
    })
}

/// Resolve a target material name.
///
/// # Errors
/// As [`find_spectrum_entry`].
pub fn resolve_material(name: &str) -> Result<ResolvedMaterial, DatasetError> {
    resolve_entry(&find_spectrum_entry(name)?)
}

/// Deterministic uniform draw over the complete built-in catalogue.
pub fn sample_target(seed: u64) -> MineralSpectrumEntry {
    let index = Prng::from_stream(seed, 0x7A12_0001).next_index(MINERAL_SPECTRUM.len());
    MINERAL_SPECTRUM[index]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_builtin_is_present_and_has_its_real_properties() {
        assert_eq!(MINERAL_SPECTRUM.len(), GemMaterial::built_in().len());
        for gem in GemMaterial::built_in() {
            let entry = find_spectrum_entry(&gem.name).unwrap();
            let resolved = resolve_entry(&entry).unwrap();
            assert_eq!(&resolved.gem, gem);
            assert!(
                (resolved.critical_angle_deg - f64::from(entry.critical_angle_deg)).abs() < 1e-4
            );
        }
        for name in ["Tanzanite", "Peridot", "Ametrine", "Ruby", "Emerald"] {
            assert!(find_spectrum_entry(name).is_ok(), "missing {name}");
        }
    }
    #[test]
    fn unknown_names_are_errors_and_sampling_is_repeatable() {
        assert!(resolve_material("Unobtainium").is_err());
        assert_eq!(find_spectrum_entry("corundum").unwrap().name, "Sapphire");
        assert_eq!(sample_target(5), sample_target(5));
    }
}
