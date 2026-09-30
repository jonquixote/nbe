//! The automation params contract, held to one verdict per fixture
//! (`tests/fixtures/automation_rules.json`). The control plane's
//! `automation.test.ts` reads the same file with its own reading
//! (`parseRule`); a rule one side accepts and the other refuses fails both —
//! the Rust→TypeScript mirror's discipline, for a contract the schema cannot
//! express.

use nbe_core::automation::{validate_rule, RuleRefs};
use nbe_core::manifest::AutomationRule;

#[test]
fn every_fixture_rule_gets_its_verdict_from_the_rust_reading() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/automation_rules.json");
    let fixture: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture readable"))
            .expect("fixture is JSON");
    let refs = RuleRefs {
        items: fixture["refs"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect(),
        bindings: fixture["refs"]["bindings"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().map(String::from)))
            .collect(),
    };
    let cases = fixture["cases"].as_array().unwrap();
    assert!(
        cases.len() >= 30,
        "the fixture must exercise every kind; has {}",
        cases.len()
    );
    let mut wrong = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let id = format!("case{i}");
        let mut rule = serde_json::json!({
            "id": id,
            "trigger": case["trigger"],
            "action": { "command": "marker.add", "payload": { "name": "m" } }
        });
        if let Some(c) = case.get("conditions") {
            rule["conditions"] = c.clone();
        }
        let rule: AutomationRule = serde_json::from_value(rule).expect("schema-shaped rule");
        let verdict = validate_rule(&rule, &refs);
        let valid = case["valid"].as_bool().unwrap();
        match (&verdict, valid) {
            (Ok(()), true) => {}
            (Err(e), false) => {
                let msg = e.to_string();
                assert!(
                    msg.starts_with(&format!("automation rule `{id}`: ")),
                    "a refusal names the rule: {msg}"
                );
            }
            _ => wrong.push(format!(
                "{id} {}: expected valid={valid}, got {verdict:?}",
                case["trigger"]
            )),
        }
    }
    assert!(
        wrong.is_empty(),
        "verdicts that disagree with the fixture:\n  {}",
        wrong.join("\n  ")
    );
}
