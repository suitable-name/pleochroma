//! `AppSettings::faceting_machine`: the cutter's faceting machine, a per-user preference.
//!
//! The machine decides the direction of the scratches a polished or ground facet shows (see
//! `pleochroma::optics::raytracer::surface::sweep`). Cutters' machines differ, so the setting
//! belongs to the user and is never stored in a design. The default is the plain machine: the
//! dop arm points at the lap centre, so the scratches run parallel to the girdle, with the
//! typical sweep. The Settings dialog (a later wave) edits it; until then the file is the way in.

use pleochroma::optics::raytracer::surface::{FacetingMachine, SweepRange};
use serde::{Deserialize, Serialize};
use std::sync::{PoisonError, RwLock};

/// The machine in force for this session, set from the settings at startup and whenever the
/// preference changes. A process-wide value rather than a parameter of every render path: the
/// library scans, the batch renders, the exports and the headless jobs all read it through
/// [`current_faceting_machine`], and it is the only per-user input of a design's render
/// physics that no design stores.
static CURRENT: RwLock<FacetingMachine> = RwLock::new(FacetingMachine::DEFAULT);

/// The faceting machine in force: the user's preference, or the default machine before the
/// settings are loaded.
#[must_use]
pub fn current_faceting_machine() -> FacetingMachine {
    *CURRENT.read().unwrap_or_else(PoisonError::into_inner)
}

/// Makes `setting` the machine in force ([`current_faceting_machine`]) and returns it.
pub fn set_current_faceting_machine(setting: &FacetingMachineSetting) -> FacetingMachine {
    let machine = setting.to_machine();
    *CURRENT.write().unwrap_or_else(PoisonError::into_inner) = machine;
    machine
}

/// The faceting machine as it is stored in the settings file.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FacetingMachineSetting {
    /// The angle in degrees between the dop arm and the radial line through the contact point.
    pub arm_offset_deg: f32,
    /// The radius of the lap in millimetres.
    pub lap_radius_mm: f32,
    /// The distance in millimetres of the line of the dop arm from the lap centre (`0` when the
    /// arm points at the centre).
    pub mast_distance_mm: f32,
    /// The smaller contact radius of the last polishing pass, in millimetres.
    pub sweep_inner_mm: f32,
    /// The larger contact radius of the last polishing pass, in millimetres.
    pub sweep_outer_mm: f32,
}

impl Default for FacetingMachineSetting {
    /// The default machine: the arm points at the lap centre, a typical sweep.
    fn default() -> Self {
        Self::from_machine(&FacetingMachine::DEFAULT)
    }
}

impl FacetingMachineSetting {
    /// The setting that stands for `machine`.
    #[must_use]
    pub const fn from_machine(machine: &FacetingMachine) -> Self {
        Self {
            arm_offset_deg: machine.arm_offset_deg,
            lap_radius_mm: machine.lap_radius_mm,
            mast_distance_mm: machine.mast_distance_mm,
            sweep_inner_mm: machine.sweep.inner_mm,
            sweep_outer_mm: machine.sweep.outer_mm,
        }
    }

    /// Whether this is the default machine (nothing worth writing to the file).
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The machine the renderer takes. A hand-edited value that is not a number, a lap without a
    /// radius or a sweep that runs backwards falls back to the default for that part, so a bad
    /// file never reaches the tracer.
    #[must_use]
    pub fn to_machine(self) -> FacetingMachine {
        let default = FacetingMachine::DEFAULT;
        let finite_or = |value: f32, fallback: f32| {
            if value.is_finite() { value } else { fallback }
        };
        let lap_radius_mm = finite_or(self.lap_radius_mm, default.lap_radius_mm);
        let lap_radius_mm = if lap_radius_mm > 0.0 {
            lap_radius_mm
        } else {
            default.lap_radius_mm
        };
        let inner = finite_or(self.sweep_inner_mm, default.sweep.inner_mm).max(0.0);
        let outer = finite_or(self.sweep_outer_mm, default.sweep.outer_mm).max(0.0);
        FacetingMachine {
            arm_offset_deg: finite_or(self.arm_offset_deg, default.arm_offset_deg)
                .clamp(-360.0, 360.0),
            lap_radius_mm,
            mast_distance_mm: finite_or(self.mast_distance_mm, default.mast_distance_mm),
            sweep: SweepRange {
                inner_mm: inner.min(outer),
                outer_mm: inner.max(outer),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::{AppSettings, SettingsFile};

    #[test]
    fn the_default_is_the_plain_machine_and_a_file_without_the_key_loads_it() {
        assert_eq!(
            AppSettings::default().faceting_machine.to_machine(),
            FacetingMachine::DEFAULT
        );
        let parsed: SettingsFile = toml::from_str("[settings]\nexposure = 1.2\n").expect("parse");
        assert!(parsed.settings.faceting_machine.is_default());
    }

    #[test]
    fn the_default_machine_is_not_written_and_a_changed_one_round_trips() {
        let text = toml::to_string(&SettingsFile::default()).expect("serialize");
        assert!(!text.contains("faceting_machine"), "{text}");
        let mut file = SettingsFile::default();
        file.settings.faceting_machine.arm_offset_deg = 12.5;
        file.settings.faceting_machine.mast_distance_mm = 30.0;
        let text = toml::to_string(&file).expect("serialize");
        let back: SettingsFile = toml::from_str(&text).expect("parse");
        assert_eq!(
            back.settings.faceting_machine,
            file.settings.faceting_machine
        );
    }

    /// The machine in force starts as the default and follows the preference once set. One test
    /// for both steps: the value is process-wide, so two tests could not run side by side.
    #[test]
    fn the_machine_in_force_follows_the_preference() {
        assert_eq!(current_faceting_machine(), FacetingMachine::DEFAULT);
        let setting = FacetingMachineSetting {
            arm_offset_deg: 12.0,
            ..FacetingMachineSetting::default()
        };
        let machine = set_current_faceting_machine(&setting);
        assert_eq!(machine.arm_offset_deg.to_bits(), 12.0f32.to_bits());
        assert_eq!(current_faceting_machine(), machine);
        set_current_faceting_machine(&FacetingMachineSetting::default());
        assert_eq!(current_faceting_machine(), FacetingMachine::DEFAULT);
    }

    #[test]
    fn a_bad_value_falls_back_to_the_default_for_that_part() {
        let bad = FacetingMachineSetting {
            arm_offset_deg: f32::NAN,
            lap_radius_mm: -3.0,
            mast_distance_mm: f32::INFINITY,
            sweep_inner_mm: 90.0,
            sweep_outer_mm: 10.0,
        };
        let machine = bad.to_machine();
        assert_eq!(machine.arm_offset_deg.to_bits(), 0.0f32.to_bits());
        assert_eq!(
            machine.lap_radius_mm.to_bits(),
            FacetingMachine::DEFAULT.lap_radius_mm.to_bits()
        );
        assert_eq!(machine.mast_distance_mm.to_bits(), 0.0f32.to_bits());
        assert!(machine.sweep.inner_mm <= machine.sweep.outer_mm);
    }
}
