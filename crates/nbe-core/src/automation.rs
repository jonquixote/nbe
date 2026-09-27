//! Automation rule parameters (SPEC §13.2 triggers).
//!
//! The manifest schema types a rule's `trigger.params` as a free object
//! (`additionalProperties: true`), and §13.2 names each trigger kind without
//! its parameters. This module is the tree's reading of those parameters for
//! the triggers that the ENGINE must evaluate. Today that is one:
//! `audioLevel`, whose crossing only the engine can see in time (SPEC v0.4.7
//! candidate B1: "the crossing is computed in the engine at render cadence").
//! Every other trigger kind is evaluated by the control plane and read there.
//!
//! A malformed `audioLevel` rule is an error, never a silently inert watch
//! (Prompt 11 §9: "Accept a trigger kind it cannot evaluate and leave the rule
//! silently inert" is forbidden). `nbe-preflight` refuses it at load with the
//! reason; the engine skips it, loudly, if one ever reaches it.

use crate::manifest::{AutomationRule, AutomationTriggerKind};
use serde::{Deserialize, Serialize};

/// Which way a bus level crosses its threshold (SPEC v0.4.7, candidate).
/// `rising`: from below to at-or-above. `falling`: from at-or-above to below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrossingDirection {
    Rising,
    Falling,
}

impl CrossingDirection {
    /// The params / wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            CrossingDirection::Rising => "rising",
            CrossingDirection::Falling => "falling",
        }
    }
}

/// The buses the engine's audio graph meters, named as `busPeakDbfs` names
/// them (the engine's `audio::BusId`, pinned against this list by
/// `prompt11_audio_level::the_bus_names_match_the_engine_graph`), plus
/// per-guest buses as `guest:<guestId>`. A rule on any other name could never
/// fire, so it is refused.
pub const AUDIO_BUSES: &[&str] = &[
    "mic",
    "clip",
    "music",
    "sfx",
    "guest",
    "master",
    "guestReturn",
    "ifb",
];

/// Whether `bus` names a metered bus: one of [`AUDIO_BUSES`], or `guest:<id>`.
pub fn is_audio_bus(bus: &str) -> bool {
    AUDIO_BUSES.contains(&bus) || bus.strip_prefix("guest:").is_some_and(|id| !id.is_empty())
}

/// The metered floor, in dBFS: the audio graph reports silence as -120
/// (`audio::linear_to_db`). A threshold at or below it could never be crossed
/// from below, so it is refused rather than accepted and inert.
pub const LEVEL_FLOOR_DBFS: f64 = -120.0;

/// One `audioLevel` watch: a bus, a threshold, a direction. Several rules
/// with the same triple share one watch — the engine reports the crossing
/// once and the control plane's evaluator fires every matching rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioLevelWatch {
    /// The bus id as `busPeakDbfs` names it — one of [`AUDIO_BUSES`] or
    /// `guest:<id>`.
    pub bus: String,
    pub threshold_dbfs: f64,
    pub direction: CrossingDirection,
}

/// Why an `audioLevel` rule's params were refused. The message names the
/// rule, so a preflight report points at the line to fix.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("automation rule `{rule}`: audioLevel trigger {reason}")]
pub struct ParamError {
    pub rule: String,
    pub reason: String,
}

/// Read one rule's `audioLevel` params. `Ok(None)` for a rule of any other
/// kind.
///
/// The params contract (the tree's reading of §13.2 — see this module's doc):
/// `{ "bus": string, "thresholdDbfs": number, "direction"?: "rising" | "falling" }`,
/// `direction` defaulting to `rising`, and no other keys. An unknown key is an
/// error, because a typo'd key (`treshold`) would otherwise leave a rule that
/// never fires.
pub fn audio_level_watch(rule: &AutomationRule) -> Result<Option<AudioLevelWatch>, ParamError> {
    if rule.trigger.kind != AutomationTriggerKind::AudioLevel {
        return Ok(None);
    }
    let err = |reason: String| ParamError {
        rule: rule.id.clone(),
        reason,
    };
    let params = rule
        .trigger
        .params
        .as_ref()
        .ok_or_else(|| err("needs params { bus, thresholdDbfs }".into()))?;
    for key in params.keys() {
        if !matches!(key.as_str(), "bus" | "thresholdDbfs" | "direction") {
            return Err(err(format!(
                "has an unknown param `{key}` (allowed: bus, thresholdDbfs, direction)"
            )));
        }
    }
    let bus = params
        .get("bus")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| err("needs a non-empty string `bus`".into()))?
        .to_string();
    if !is_audio_bus(&bus) {
        return Err(err(format!(
            "names bus `{bus}`, which the audio graph does not meter (buses: {}, or guest:<id>)",
            AUDIO_BUSES.join(", ")
        )));
    }
    let threshold_dbfs = params
        .get("thresholdDbfs")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| err("needs a number `thresholdDbfs`".into()))?;
    if !(threshold_dbfs > LEVEL_FLOOR_DBFS && threshold_dbfs <= 0.0) {
        return Err(err(format!(
            "thresholdDbfs {threshold_dbfs} is outside ({LEVEL_FLOOR_DBFS}, 0]: \
             the meter floor is {LEVEL_FLOOR_DBFS} dBFS and full scale is 0"
        )));
    }
    let direction = match params.get("direction") {
        None => CrossingDirection::Rising,
        Some(v) => match v.as_str() {
            Some("rising") => CrossingDirection::Rising,
            Some("falling") => CrossingDirection::Falling,
            _ => {
                return Err(err(format!(
                    "direction {v} is not \"rising\" or \"falling\""
                )))
            }
        },
    };
    Ok(Some(AudioLevelWatch {
        bus,
        threshold_dbfs,
        direction,
    }))
}

/// Every `audioLevel` watch the rules ask for, deduplicated, in first-seen
/// order, and the errors for rules whose params were refused. `enabled` does
/// not filter: enabling is the control plane's toggle at runtime
/// (`automation.enable`), and a watch costs one comparison per block.
pub fn audio_level_watches(rules: &[AutomationRule]) -> (Vec<AudioLevelWatch>, Vec<ParamError>) {
    let mut watches: Vec<AudioLevelWatch> = Vec::new();
    let mut errors = Vec::new();
    for rule in rules {
        match audio_level_watch(rule) {
            Ok(Some(w)) => {
                if !watches.contains(&w) {
                    watches.push(w);
                }
            }
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    (watches, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, params: serde_json::Value) -> AutomationRule {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "trigger": { "kind": "audioLevel", "params": params },
            "action": { "command": "marker.add", "payload": { "name": "hot" } }
        }))
        .unwrap()
    }

    #[test]
    fn direction_tokens_are_stable() {
        assert_eq!(CrossingDirection::Rising.as_str(), "rising");
        assert_eq!(CrossingDirection::Falling.as_str(), "falling");
        assert_eq!(
            serde_json::to_value(CrossingDirection::Falling).unwrap(),
            serde_json::json!("falling")
        );
    }

    #[test]
    fn a_complete_audio_level_rule_reads_with_rising_as_the_default() {
        let w = audio_level_watch(&rule(
            "r1",
            serde_json::json!({ "bus": "mic", "thresholdDbfs": -12 }),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(
            w,
            AudioLevelWatch {
                bus: "mic".into(),
                threshold_dbfs: -12.0,
                direction: CrossingDirection::Rising
            }
        );
    }

    #[test]
    fn a_malformed_audio_level_rule_is_refused_with_its_reason() {
        for (params, needle) in [
            (serde_json::json!({ "thresholdDbfs": -12 }), "bus"),
            (serde_json::json!({ "bus": "mic" }), "thresholdDbfs"),
            (
                serde_json::json!({ "bus": "mic", "thresholdDbfs": 3 }),
                "outside",
            ),
            (
                serde_json::json!({ "bus": "mic", "thresholdDbfs": -120 }),
                "outside",
            ),
            (
                serde_json::json!({ "bus": "mic", "thresholdDbfs": -6, "direction": "up" }),
                "direction",
            ),
            (
                serde_json::json!({ "bus": "mic", "treshold": -6 }),
                "unknown param `treshold`",
            ),
            (
                serde_json::json!({ "bus": "program", "thresholdDbfs": -6 }),
                "does not meter",
            ),
            (
                serde_json::json!({ "bus": "guest:", "thresholdDbfs": -6 }),
                "does not meter",
            ),
        ] {
            let e = audio_level_watch(&rule("bad", params.clone())).expect_err(&params.to_string());
            let msg = e.to_string();
            assert!(msg.contains("rule `bad`"), "{msg}");
            assert!(msg.contains(needle), "{params}: {msg}");
        }
    }

    #[test]
    fn watches_deduplicate_and_other_kinds_are_not_watches() {
        let mut rules = vec![
            rule(
                "a",
                serde_json::json!({ "bus": "mic", "thresholdDbfs": -12 }),
            ),
            rule(
                "b",
                serde_json::json!({ "bus": "mic", "thresholdDbfs": -12, "direction": "rising" }),
            ),
            rule(
                "c",
                serde_json::json!({ "bus": "mic", "thresholdDbfs": -12, "direction": "falling" }),
            ),
        ];
        rules.push(
            serde_json::from_value(serde_json::json!({
                "id": "t", "trigger": { "kind": "timer", "params": { "atFrame": 30 } },
                "action": { "command": "marker.add" }
            }))
            .unwrap(),
        );
        let (w, e) = audio_level_watches(&rules);
        assert!(e.is_empty());
        assert_eq!(w.len(), 2, "a and b share one watch; c falls: {w:?}");
    }
}
