//! The cutter's orientation settings of a plan: which rule applies, the overrides, the Reasons.
//!
//! [`PlanSettingsOverrides`] is the part of the planner's state that is not the rough: the window
//! keeps it beside the `RoughModel` and records both in ONE undo entry, so every rule edit and
//! every override is one undo step. It is plain data (no clock, no I/O) and its comparison is
//! the undo test.
//!
//! # Which rule applies
//!
//! 1. No rule is ever applied while the rough's orientation is Unknown.
//! 2. The plan's rule is the material's default ([`default_rule`], the D3 table) until the cutter
//!    changes it: [`RuleChoice::Default`]. The default is switched on by the rough getting an
//!    orientation, never silently before ([`PlanSettingsOverrides::rule_line`] is the visible
//!    line the window shows).
//! 3. [`RuleChoice::Ignore`] is "None (Ignore the Axis)", [`RuleChoice::Set`] a rule of the
//!    cutter's own (a kind, a tolerance, Preferred or Required).
//! 4. A design may override the plan's rule the same way ([`RuleChoice::Default`] there means
//!    "follow the plan").
//!
//! Each setting carries an optional Reason, saved with the plan and printed on the cutting
//! sheet.

use std::collections::BTreeMap;

use pleochroma::optics::materials::GemMaterial;

use super::{ResolvedRule, StoneChoice};
use crate::{
    crystal_axis::{CrystalOrientation, OrientationRule, OrientationRuleKind, default_rule},
    rough_plan::{CandidateDesign, placement::DesignPlacement},
};

/// What a setting says about the rule.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum RuleChoice {
    /// The level above: the material's default rule for the plan, the plan's rule for a design.
    #[default]
    Default,
    /// No rule: the axis is ignored ("None (Ignore the Axis)").
    Ignore,
    /// This rule.
    Set(OrientationRule),
}

/// A rule setting with its optional Reason (empty when none was given).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuleSetting {
    /// What the setting says.
    pub choice: RuleChoice,
    /// Why the cutter changed it; empty when no reason was given.
    pub reason: String,
}

impl RuleSetting {
    /// Whether the setting is the untouched default (nothing to save).
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.choice == RuleChoice::Default && self.reason.is_empty()
    }
}

/// Where the plan's rule comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleSource {
    /// The material's default rule (plan decision D3).
    MaterialDefault,
    /// A rule the cutter set.
    Own,
}

/// The line the planner window shows once the rough has an orientation and a rule applies.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleLine {
    /// The sentence: "Stones are oriented to the crystal axis: Table Across (Sapphire default)".
    pub text: String,
    /// Where the rule comes from.
    pub source: RuleSource,
    /// The rule.
    pub rule: OrientationRule,
}

/// What resolving the rules of a plan needs besides the settings.
#[derive(Debug, Clone, Copy)]
pub struct ResolveContext<'a> {
    /// The rough's orientation, `None` while it is Unknown (no rule then applies).
    pub rough: Option<CrystalOrientation>,
    /// The plan's material (its default rule, and the principal directions of a color-named
    /// rule).
    pub material: &'a GemMaterial,
    /// The placement of each design (by entry id), used by a rule with an azimuth target. A
    /// design without an entry is placed by the identity.
    pub placements: &'a BTreeMap<i64, DesignPlacement>,
}

/// The orientation settings and overrides of a plan, undone and redone as one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlanSettingsOverrides {
    /// The plan's rule setting.
    pub plan: RuleSetting,
    /// Per-design rule settings by library entry id (only designs the cutter changed).
    pub designs: BTreeMap<i64, RuleSetting>,
    /// The cutter's per-stone choices, by `(result, stone)` as the results list them (only
    /// stones the cutter changed; see [`StoneChoice::is_default`]).
    pub stones: BTreeMap<(u32, u32), StoneChoice>,
}

impl PlanSettingsOverrides {
    /// Whether nothing was changed: no plan rule, no design rule, no Reason, no stone choice.
    /// Such a plan saves exactly as it always did.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.plan.is_default() && self.designs.is_empty() && self.stones.is_empty()
    }

    /// The plan's rule: the material's default or the cutter's own, `None` while the rough's
    /// orientation is Unknown, when the cutter chose to ignore the axis, or when the material
    /// has no rule.
    #[must_use]
    pub fn plan_rule(
        &self,
        rough: Option<&CrystalOrientation>,
        material: &GemMaterial,
    ) -> Option<OrientationRule> {
        rough?;
        let rule = match self.plan.choice {
            RuleChoice::Default => default_rule(material),
            RuleChoice::Ignore => None,
            RuleChoice::Set(rule) => Some(rule),
        };
        rule.filter(|rule| rule.kind != OrientationRuleKind::None)
    }

    /// The rule design `entry_id` is cut under: its own setting, else `plan_rule`.
    #[must_use]
    pub fn design_rule(
        &self,
        entry_id: i64,
        plan_rule: Option<OrientationRule>,
    ) -> Option<OrientationRule> {
        let rule = match self.designs.get(&entry_id).map(|setting| setting.choice) {
            None | Some(RuleChoice::Default) => plan_rule,
            Some(RuleChoice::Ignore) => None,
            Some(RuleChoice::Set(rule)) => Some(rule),
        };
        rule.filter(|rule| rule.kind != OrientationRuleKind::None)
    }

    /// The rule design `entry_id` is cut under in `ctx` (what the rows compare its stones with).
    #[must_use]
    pub fn effective_rule(
        &self,
        entry_id: i64,
        ctx: &ResolveContext<'_>,
    ) -> Option<OrientationRule> {
        let plan = self.plan_rule(ctx.rough.as_ref(), ctx.material);
        ctx.rough?;
        self.design_rule(entry_id, plan)
    }

    /// The line the window shows ("Stones are oriented to the crystal axis: Table Across
    /// (Sapphire default)"), or `None` while no rule applies (the orientation is Unknown, the
    /// axis is ignored, or the material has no rule).
    #[must_use]
    pub fn rule_line(
        &self,
        rough: Option<&CrystalOrientation>,
        material: &GemMaterial,
    ) -> Option<RuleLine> {
        let rule = self.plan_rule(rough, material)?;
        let (source, origin) = match self.plan.choice {
            RuleChoice::Set(_) => (RuleSource::Own, "your setting".to_string()),
            _ => (
                RuleSource::MaterialDefault,
                format!("{} default", material.name),
            ),
        };
        Some(RuleLine {
            text: format!(
                "Stones are oriented to the crystal axis: {} ({origin})",
                rule.name()
            ),
            source,
            rule,
        })
    }

    /// Gives every design of `designs` the resolved rule it is cut under (or `None`), so the
    /// next plan sees the rules. With no rough orientation, or no rule, every design gets
    /// `None` and the plan is the plan it always was.
    pub fn resolve_designs(&self, designs: &mut [CandidateDesign], ctx: &ResolveContext<'_>) {
        let plan_rule = self.plan_rule(ctx.rough.as_ref(), ctx.material);
        for design in designs {
            design.orientation = ctx.rough.and_then(|orientation| {
                let rule = self.design_rule(design.entry_id, plan_rule)?;
                let resolved = ResolvedRule::from_rule(&rule, orientation, ctx.material)?;
                let placement = ctx
                    .placements
                    .get(&design.entry_id)
                    .copied()
                    .unwrap_or(DesignPlacement::IDENTITY);
                Some(resolved.with_placement(placement))
            });
        }
    }

    /// The Reason given for design `entry_id`'s rule, if any.
    #[must_use]
    pub fn design_reason(&self, entry_id: i64) -> Option<&str> {
        self.designs
            .get(&entry_id)
            .map(|setting| setting.reason.as_str())
            .filter(|reason| !reason.is_empty())
    }

    /// The Reason given for the plan's rule, if any.
    #[must_use]
    pub fn plan_reason(&self) -> Option<&str> {
        Some(self.plan.reason.as_str()).filter(|reason| !reason.is_empty())
    }

    /// Sets the plan's rule setting (one undo step for the window).
    pub fn set_plan(&mut self, setting: RuleSetting) {
        self.plan = setting;
    }

    /// Sets design `entry_id`'s rule setting; the untouched default removes the entry.
    pub fn set_design(&mut self, entry_id: i64, setting: RuleSetting) {
        if setting.is_default() {
            self.designs.remove(&entry_id);
        } else {
            self.designs.insert(entry_id, setting);
        }
    }

    /// The cutter's choice for stone `stone` of result `layout`, if any.
    #[must_use]
    pub fn stone_choice(&self, layout: u32, stone: u32) -> Option<&StoneChoice> {
        self.stones.get(&(layout, stone))
    }

    /// Sets the choice for stone `stone` of result `layout`; a choice that changes nothing
    /// removes the entry.
    pub fn set_stone_choice(&mut self, layout: u32, stone: u32, choice: StoneChoice) {
        if choice.is_default() {
            self.stones.remove(&(layout, stone));
        } else {
            self.stones.insert((layout, stone), choice);
        }
    }
}
