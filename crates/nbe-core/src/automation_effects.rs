//! SPEC §13.4 — preflight's static cycle check, over §13.4.1 as data.
//!
//! "Preflight MUST statically reject rules whose action can re-trigger
//! themselves directly or transitively" (§13.4; AC-25 #3). Which command can
//! cause which trigger is §13.4.1's table, held here as data
//! (`automation_effects.json`) so that one copy serves two readers: this
//! module, and the control plane's runtime check, which executes each command
//! and asserts the table's same-dispatch cells against what its evaluator
//! actually raises (`packages/control-plane/src/automation.test.ts`).
//!
//! The graph: one node per rule, disabled rules included (`automation.enable`
//! can arm one at runtime). An edge `a → b` exists when `a`'s action can cause
//! `b`'s trigger. Three readings, each over-approximating, because a spurious
//! edge refuses a rule that would not loop while a missing one admits a loop
//! (§13.4.1's "safe direction"):
//!
//! * **Conditions are ignored.** A condition that happens to break the loop
//!   at runtime does not break it here.
//! * **Deferred edges count** (Prompt 11 WU5's decision, which §13.4.1 left
//!   open). A `mediaEnd` rule whose action takes a timed item re-triggers
//!   itself one duration later. AC-25 #3 names self-triggering rules without
//!   an exception for timing, and the cause chain that the runtime suppression
//!   reads does not survive the delay, so only this check sees that loop. A
//!   looping playlist is therefore refused.
//! * **Items narrow only where the payload names one.** `view.cut
//!   { itemRef }` can start and end only that item; `view.take` takes the
//!   preview, which preflight cannot know, so it can start any. `itemState` is
//!   never narrowed: a cut also returns the previous live item to `READY`.

use crate::manifest::{AutomationRule, AutomationTriggerKind};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// §13.4.1 as data — the file's `$comment` defines each field.
pub const EFFECTS_JSON: &str = include_str!("automation_effects.json");

/// One command's row in [`EFFECTS_JSON`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandEffects {
    /// The `stateChange` fields it can change in its own dispatch.
    #[serde(rename = "stateChange", default)]
    pub state_change: Vec<String>,
    /// It can put an item on air in its own dispatch (B3's `mediaStart`).
    #[serde(rename = "mediaStart", default)]
    pub media_start: bool,
    /// Trigger kinds it can cause later, outside its dispatch.
    #[serde(default)]
    pub deferred: Vec<String>,
    /// The payload key naming the one item whose start and end it can cause.
    #[serde(default)]
    pub item: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectsFile {
    #[serde(rename = "$comment")]
    _comment: serde_json::Value,
    commands: BTreeMap<String, CommandEffects>,
}

/// Every §16 command's effects, parsed once.
pub fn command_effects() -> &'static BTreeMap<String, CommandEffects> {
    static EFFECTS: OnceLock<BTreeMap<String, CommandEffects>> = OnceLock::new();
    EFFECTS.get_or_init(|| {
        serde_json::from_str::<EffectsFile>(EFFECTS_JSON)
            .expect("automation_effects.json parses")
            .commands
    })
}

/// Why `from`'s action can raise `to`'s trigger, or `None` when it cannot.
/// `from.action.command` must be canonical (preflight resolves aliases
/// first); a command with no row causes nothing — preflight has already
/// refused it as not a command.
pub fn edge(from: &AutomationRule, to: &AutomationRule) -> Option<String> {
    let cmd = from.action.command.as_str();
    let eff = command_effects().get(cmd)?;
    let param = |k: &str| {
        to.trigger
            .params
            .as_ref()
            .and_then(|p| p.get(k))
            .and_then(|v| v.as_str())
    };
    let named_item = eff
        .item
        .as_deref()
        .and_then(|k| from.action.payload.as_ref()?.get(k)?.as_str());
    let item_can_match = match (param("itemRef"), named_item) {
        (Some(want), Some(got)) => want == got,
        _ => true,
    };
    let deferred = |k: &str| eff.deferred.iter().any(|d| d == k);
    match to.trigger.kind {
        AutomationTriggerKind::StateChange => {
            let field = param("field")?;
            eff.state_change
                .iter()
                .any(|f| f == field)
                .then(|| format!("`{cmd}` changes `{field}`"))
        }
        AutomationTriggerKind::MediaStart => (eff.media_start && item_can_match)
            .then(|| format!("`{cmd}` puts an item on air (mediaStart)")),
        AutomationTriggerKind::MediaEnd => (deferred("mediaEnd") && item_can_match)
            .then(|| format!("`{cmd}` schedules an item's end (mediaEnd, deferred)")),
        AutomationTriggerKind::Timer => {
            deferred("timer").then(|| format!("`{cmd}` starts the show clock (timer, deferred)"))
        }
        AutomationTriggerKind::StreamHealth => deferred("streamHealth")
            .then(|| format!("`{cmd}` moves the stream transport (streamHealth, deferred)")),
        AutomationTriggerKind::AudioLevel => deferred("audioLevel")
            .then(|| format!("`{cmd}` moves a bus level (audioLevel, deferred)")),
        AutomationTriggerKind::Hotkey
        | AutomationTriggerKind::TimeOfDay
        | AutomationTriggerKind::RssKeyword => None,
    }
}

/// One step of a cycle: the rule, and why its action raises the next rule's
/// trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleStep {
    pub rule: String,
    pub because: String,
}

/// The first rule cycle, searching from rules in manifest order: the steps
/// in order, the last step's action raising the first step's trigger. `None`
/// when the rules form no cycle.
pub fn find_cycle(rules: &[AutomationRule]) -> Option<Vec<CycleStep>> {
    let n = rules.len();
    let adj: Vec<Vec<(usize, String)>> = (0..n)
        .map(|a| {
            (0..n)
                .filter_map(|b| edge(&rules[a], &rules[b]).map(|why| (b, why)))
                .collect()
        })
        .collect();
    // 0 = unvisited, 1 = on the stack, 2 = done.
    let mut color = vec![0u8; n];
    let mut stack: Vec<(usize, String)> = Vec::new();
    fn visit(
        v: usize,
        adj: &[Vec<(usize, String)>],
        color: &mut [u8],
        stack: &mut Vec<(usize, String)>,
    ) -> Option<Vec<(usize, String)>> {
        color[v] = 1;
        for (w, why) in &adj[v] {
            stack.push((v, why.clone()));
            if color[*w] == 1 {
                let start = stack.iter().position(|(u, _)| *u == *w)?;
                return Some(stack[start..].to_vec());
            }
            if color[*w] == 0 {
                if let Some(c) = visit(*w, adj, color, stack) {
                    return Some(c);
                }
            }
            stack.pop();
        }
        color[v] = 2;
        None
    }
    for v in 0..n {
        if color[v] == 0 {
            if let Some(c) = visit(v, &adj, &mut color, &mut stack) {
                return Some(
                    c.into_iter()
                        .map(|(i, because)| CycleStep {
                            rule: rules[i].id.clone(),
                            because,
                        })
                        .collect(),
                );
            }
        }
    }
    None
}

/// The refusal, naming the cycle: "`a` → `b` → `a` (`a`: `view.cut` puts an
/// item on air (mediaStart); `b`: …)".
pub fn describe_cycle(cycle: &[CycleStep]) -> String {
    let mut path: Vec<String> = cycle.iter().map(|s| format!("`{}`", s.rule)).collect();
    if let Some(first) = cycle.first() {
        path.push(format!("`{}`", first.rule));
    }
    let why: Vec<String> = cycle
        .iter()
        .map(|s| format!("`{}`: {}", s.rule, s.because))
        .collect();
    format!(
        "automation rules form a cycle: {} ({}) — a rule's action can re-trigger itself",
        path.join(" → "),
        why.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, trigger: serde_json::Value, action: serde_json::Value) -> AutomationRule {
        serde_json::from_value(
            serde_json::json!({ "id": id, "trigger": trigger, "action": action }),
        )
        .unwrap()
    }

    fn ids(c: &[CycleStep]) -> Vec<&str> {
        c.iter().map(|s| s.rule.as_str()).collect()
    }

    #[test]
    fn the_effects_data_parses_and_names_only_state_fields() {
        let e = command_effects();
        assert_eq!(e.len(), 55, "§16's 55 commands, each once");
        for (cmd, eff) in e {
            for f in &eff.state_change {
                assert!(
                    crate::automation::STATE_FIELDS.contains(&f.as_str()),
                    "{cmd}: `{f}` is not a stateChange field"
                );
            }
            for d in &eff.deferred {
                assert!(
                    ["mediaEnd", "timer", "streamHealth", "audioLevel"].contains(&d.as_str()),
                    "{cmd}: `{d}` is not a deferred trigger kind"
                );
            }
        }
    }

    #[test]
    fn a_rule_whose_action_raises_its_own_trigger_is_a_cycle_of_one() {
        let r = rule(
            "loop",
            serde_json::json!({ "kind": "stateChange", "params": { "field": "previewItem" } }),
            serde_json::json!({ "command": "preview.set", "payload": { "itemRef": "A1" } }),
        );
        let c = find_cycle(&[r]).expect("preview.set changes previewItem");
        assert_eq!(ids(&c), ["loop"]);
        assert_eq!(
            describe_cycle(&c),
            "automation rules form a cycle: `loop` → `loop` (`loop`: `preview.set` changes `previewItem`) — a rule's action can re-trigger itself"
        );
    }

    #[test]
    fn a_transitive_cycle_is_found_and_named_in_order() {
        // Neither rule raises its own trigger; together they loop.
        let rules = [
            rule(
                "a",
                serde_json::json!({ "kind": "stateChange", "params": { "field": "recordState" } }),
                serde_json::json!({ "command": "view.take" }),
            ),
            rule(
                "b",
                serde_json::json!({ "kind": "stateChange", "params": { "field": "viewItem" } }),
                serde_json::json!({ "command": "record.start" }),
            ),
        ];
        let c =
            find_cycle(&rules).expect("take changes viewItem; record.start changes recordState");
        assert_eq!(ids(&c), ["a", "b"]);
        assert_eq!(c[0].because, "`view.take` changes `viewItem`");
        assert_eq!(c[1].because, "`record.start` changes `recordState`");
    }

    #[test]
    fn a_linear_playlist_is_not_a_cycle_and_a_looping_one_is() {
        let end =
            |item: &str| serde_json::json!({ "kind": "mediaEnd", "params": { "itemRef": item } });
        let cut = |item: &str| serde_json::json!({ "command": "view.cut", "payload": { "itemRef": item } });
        let linear = [
            rule("ab", end("A"), cut("B")),
            rule("bc", end("B"), cut("C")),
        ];
        assert_eq!(
            find_cycle(&linear),
            None,
            "cut B ends only B; no rule leads back"
        );
        let looping = [
            rule("ab", end("A"), cut("B")),
            rule("ba", end("B"), cut("A")),
        ];
        assert_eq!(
            find_cycle(&looping).map(|c| ids(&c).join(",")),
            Some("ab,ba".into()),
            "deferred edges count (WU5): a looping playlist re-triggers itself"
        );
    }

    #[test]
    fn frame_only_actions_and_operator_only_triggers_have_no_edges() {
        let marker = serde_json::json!({ "command": "marker.add", "payload": { "name": "m" } });
        let rules = [
            rule(
                "m",
                serde_json::json!({ "kind": "stateChange", "params": { "field": "showState" } }),
                marker,
            ),
            rule(
                "h",
                serde_json::json!({ "kind": "hotkey", "params": { "bindingId": "k" } }),
                serde_json::json!({ "command": "view.take" }),
            ),
        ];
        assert_eq!(find_cycle(&rules), None);
    }
}
