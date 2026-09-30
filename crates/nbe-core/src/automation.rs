//! Automation rule parameters (SPEC §13.2 triggers).
//!
//! The manifest schema types a rule's `trigger.params` and `conditions` as
//! free objects (`additionalProperties: true`), and §13.2 names each trigger
//! kind without its parameters. This module is the tree's reading of them —
//! recorded in `docs/automation-design.md` — in two parts:
//!
//! * [`validate_rule`] reads every trigger kind and every condition, and is
//!   what `nbe-preflight` refuses a package by (the validation decision is
//!   preflight's, Addendum 02a §1.4). The control plane's evaluator reads the
//!   same contract (`packages/control-plane/src/automation.ts`), and the two
//!   are held to one verdict per fixture by
//!   `tests/fixtures/automation_rules.json`.
//! * [`audio_level_watch`] extracts what the ENGINE evaluates: `audioLevel`,
//!   whose crossing only the engine can see in time (SPEC v0.4.7 candidate B1:
//!   "the crossing is computed in the engine at render cadence").
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
#[error("automation rule `{rule}`: {reason}")]
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
    audio_level_params(rule).map_err(|reason| ParamError {
        rule: rule.id.clone(),
        reason: format!("audioLevel trigger {reason}"),
    })
}

/// [`audio_level_watch`]'s reading, with the bare reason on refusal.
fn audio_level_params(rule: &AutomationRule) -> Result<Option<AudioLevelWatch>, String> {
    if rule.trigger.kind != AutomationTriggerKind::AudioLevel {
        return Ok(None);
    }
    let err = |reason: String| reason;
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

/// What a rule may reference in its package: rundown item ids, and control
/// bindings with their trigger kinds.
#[derive(Debug, Default, Clone)]
pub struct RuleRefs {
    pub items: std::collections::BTreeSet<String>,
    /// binding id → its `trigger.kind` (`hotkey`, `companionKey`, …), `None`
    /// for a binding with no trigger.
    pub bindings: std::collections::BTreeMap<String, Option<String>>,
}

/// The control-plane state a `stateChange` trigger or a condition can name.
pub const STATE_FIELDS: &[&str] = &[
    "showState",
    "viewItem",
    "previewItem",
    "streamState",
    "recordState",
    "automationHold",
    "fallbackActive",
    "itemState",
];

type Params = std::collections::HashMap<String, serde_json::Value>;

fn only_keys(params: &Params, allowed: &[&str]) -> Result<(), String> {
    let mut keys: Vec<&String> = params.keys().collect();
    keys.sort();
    for k in keys {
        if !allowed.contains(&k.as_str()) {
            return Err(format!(
                "unknown param `{k}` (allowed: {})",
                if allowed.is_empty() {
                    "none".to_string()
                } else {
                    allowed.join(", ")
                }
            ));
        }
    }
    Ok(())
}

fn trigger_reason(rule: &AutomationRule, refs: &RuleRefs) -> Result<(), String> {
    let empty = Params::new();
    let params = rule.trigger.params.as_ref().unwrap_or(&empty);
    let item = |v: Option<&serde_json::Value>| -> bool {
        v.and_then(|v| v.as_str())
            .is_some_and(|s| refs.items.contains(s))
    };
    match rule.trigger.kind {
        AutomationTriggerKind::MediaEnd | AutomationTriggerKind::MediaStart => {
            only_keys(params, &["itemRef"])?;
            match params.get("itemRef") {
                None => Ok(()),
                Some(v) if item(Some(v)) => Ok(()),
                Some(v) => Err(format!("itemRef {v} is not an item in the rundown")),
            }
        }
        AutomationTriggerKind::Timer => {
            only_keys(params, &["atMs"])?;
            match params.get("atMs").and_then(|v| v.as_f64()) {
                Some(ms) if ms.is_finite() && ms > 0.0 => Ok(()),
                _ => Err(
                    "needs a positive number `atMs` (show-clock milliseconds after show.start)"
                        .into(),
                ),
            }
        }
        AutomationTriggerKind::TimeOfDay => {
            only_keys(params, &["at"])?;
            let ok = params.get("at").and_then(|v| v.as_str()).is_some_and(|s| {
                let parts: Vec<&str> = s.split(':').collect();
                let two = |p: &str| p.len() == 2 && p.bytes().all(|b| b.is_ascii_digit());
                (parts.len() == 2 || parts.len() == 3)
                    && parts.iter().all(|p| two(p))
                    && parts[0].parse::<u32>().is_ok_and(|h| h <= 23)
                    && parts[1].parse::<u32>().is_ok_and(|m| m <= 59)
                    && parts
                        .get(2)
                        .is_none_or(|s| s.parse::<u32>().is_ok_and(|s| s <= 59))
            });
            if ok {
                Ok(())
            } else {
                Err("needs `at` as \"HH:mm\" or \"HH:mm:ss\" (local wall clock)".into())
            }
        }
        AutomationTriggerKind::AudioLevel => audio_level_params(rule).map(|_| ()),
        AutomationTriggerKind::Hotkey => {
            only_keys(params, &["bindingId"])?;
            let Some(id) = params.get("bindingId").and_then(|v| v.as_str()) else {
                return Err(format!(
                    "bindingId {} is not a control binding in this package",
                    params
                        .get("bindingId")
                        .map(|v| v.to_string())
                        .unwrap_or("undefined".into())
                ));
            };
            match refs.bindings.get(id) {
                None => Err(format!(
                    "bindingId \"{id}\" is not a control binding in this package"
                )),
                Some(Some(k)) if k == "hotkey" => Ok(()),
                Some(k) => Err(format!(
                    "binding \"{id}\" is a {} binding, not a hotkey",
                    k.as_deref().unwrap_or("trigger-less")
                )),
            }
        }
        AutomationTriggerKind::RssKeyword => {
            Err("has no source in this build: no RSS feed is ever fetched \
             (ticker.refreshRss mutates nothing; SPEC §13.4.1)"
                .into())
        }
        AutomationTriggerKind::StreamHealth => {
            only_keys(params, &["state"])?;
            match params.get("state").and_then(|v| v.as_str()) {
                Some("live" | "reconnecting" | "closed") => Ok(()),
                _ => Err("needs `state`: \"live\", \"reconnecting\" or \"closed\" \
                          (a streamTransportState token; \"none\" is a stub, not a state)"
                    .into()),
            }
        }
        AutomationTriggerKind::StateChange => {
            only_keys(params, &["field", "itemRef", "from", "to"])?;
            let field = params.get("field").and_then(|v| v.as_str());
            let Some(field) = field.filter(|f| STATE_FIELDS.contains(f)) else {
                return Err(format!(
                    "field {} is not one of {}",
                    params
                        .get("field")
                        .map(|v| v.to_string())
                        .unwrap_or("undefined".into()),
                    STATE_FIELDS.join(", ")
                ));
            };
            if field == "itemState" {
                if !item(params.get("itemRef")) {
                    return Err("field itemState needs an `itemRef` naming a rundown item".into());
                }
            } else if params.contains_key("itemRef") {
                return Err("itemRef applies only to field itemState".into());
            }
            Ok(())
        }
    }
}

fn condition_reason(
    c: &std::collections::HashMap<String, serde_json::Value>,
    refs: &RuleRefs,
) -> Result<(), String> {
    only_keys(c, &["field", "itemRef", "equals"]).map_err(|e| format!("condition has an {e}"))?;
    let Some(field) = c
        .get("field")
        .and_then(|v| v.as_str())
        .filter(|f| STATE_FIELDS.contains(f))
    else {
        return Err(format!(
            "condition field {} is not one of {}",
            c.get("field")
                .map(|v| v.to_string())
                .unwrap_or("undefined".into()),
            STATE_FIELDS.join(", ")
        ));
    };
    if !c.contains_key("equals") {
        return Err("condition needs `equals`".into());
    }
    if field == "itemState" {
        let ok = c
            .get("itemRef")
            .and_then(|v| v.as_str())
            .is_some_and(|s| refs.items.contains(s));
        if !ok {
            return Err("condition on itemState needs an `itemRef` naming a rundown item".into());
        }
    } else if c.contains_key("itemRef") {
        return Err("condition itemRef applies only to field itemState".into());
    }
    Ok(())
}

/// Read one rule's trigger params and conditions against the tree's contract
/// (module doc). `Err` names the rule and the reason, in the same words the
/// control plane's reading uses. The action (a command) is checked by
/// `nbe-preflight` beside the control bindings' actions, with the same
/// registered-command list and required keys.
pub fn validate_rule(rule: &AutomationRule, refs: &RuleRefs) -> Result<(), ParamError> {
    let kind = serde_json::to_value(rule.trigger.kind)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    trigger_reason(rule, refs).map_err(|reason| ParamError {
        rule: rule.id.clone(),
        reason: format!("{kind} trigger {reason}"),
    })?;
    for c in &rule.conditions {
        condition_reason(c, refs).map_err(|reason| ParamError {
            rule: rule.id.clone(),
            reason,
        })?;
    }
    Ok(())
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
