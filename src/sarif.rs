use crate::engine::ReviewResult;
use serde::Serialize;
use std::collections::BTreeMap;

/// Convert a `ReviewResult` to SARIF v2.1.0 JSON value.
pub fn to_sarif_value(result: &ReviewResult) -> serde_json::Value {
    let mut rules: BTreeMap<String, usize> = BTreeMap::new();
    let mut sarif_results: Vec<SarifResult> = Vec::with_capacity(result.findings.len());

    for finding in &result.findings {
        let rule_idx = rules.len();
        let idx = *rules.entry(finding.category.clone()).or_insert(rule_idx);

        let level = match finding.severity.as_str() {
            "high" => "error",
            "medium" => "warning",
            "low" => "note",
            _ => "none",
        };

        let location = finding.file.as_ref().map(|file| SarifLocation {
            physical_location: PhysicalLocation {
                artifact_location: ArtifactLocation { uri: file.clone() },
                region: finding.line.map(|line| Region { start_line: line }),
            },
        });

        let fix = finding.suggestion.as_ref().map(|suggestion| SarifFix {
            description: Description {
                text: suggestion.clone(),
            },
        });

        sarif_results.push(SarifResult {
            rule_id: finding.category.clone(),
            rule_index: idx,
            level: level.to_string(),
            message: Description {
                text: finding.message.clone(),
            },
            locations: location.into_iter().collect(),
            fixes: fix.into_iter().collect(),
        });
    }

    let rules_array: Vec<SarifRule> = rules
        .into_iter()
        .map(|(id, idx)| SarifRule {
            id,
            index: idx,
            short_description: Description {
                text: String::new(),
            },
        })
        .collect();

    let run = SarifRun {
        tool: Tool {
            driver: Driver {
                name: "reviewer".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                information_uri: "https://github.com/devstroop/reviewer".into(),
                rules: rules_array,
            },
        },
        results: sarif_results,
        invocations: vec![SarifInvocation {
            execution_successful: true,
        }],
        properties: SarifRunProperties {
            review_text: result.review_text.clone(),
            stats: serde_json::to_value(&result.stats).unwrap_or_default(),
        },
    };

    serde_json::json!({
        "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json",
        "version": "2.1.0",
        "runs": [run]
    })
}

// ── SARIF types ──────────────────────────────────────────────

#[derive(Serialize)]
struct SarifRun {
    tool: Tool,
    results: Vec<SarifResult>,
    invocations: Vec<SarifInvocation>,
    properties: SarifRunProperties,
}

#[derive(Serialize)]
struct Tool {
    driver: Driver,
}

#[derive(Serialize)]
struct Driver {
    name: String,
    version: String,
    #[serde(rename = "informationUri")]
    information_uri: String,
    rules: Vec<SarifRule>,
}

#[derive(Serialize)]
struct SarifRule {
    id: String,
    index: usize,
    #[serde(rename = "shortDescription")]
    short_description: Description,
}

#[derive(Serialize)]
struct SarifResult {
    #[serde(rename = "ruleId")]
    rule_id: String,
    #[serde(rename = "ruleIndex")]
    rule_index: usize,
    level: String,
    message: Description,
    locations: Vec<SarifLocation>,
    fixes: Vec<SarifFix>,
}

#[derive(Serialize)]
struct Description {
    text: String,
}

#[derive(Serialize)]
struct SarifLocation {
    #[serde(rename = "physicalLocation")]
    physical_location: PhysicalLocation,
}

#[derive(Serialize)]
struct PhysicalLocation {
    #[serde(rename = "artifactLocation")]
    artifact_location: ArtifactLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    region: Option<Region>,
}

#[derive(Serialize)]
struct ArtifactLocation {
    uri: String,
}

#[derive(Serialize)]
struct Region {
    #[serde(rename = "startLine")]
    start_line: u64,
}

#[derive(Serialize)]
struct SarifFix {
    description: Description,
}

#[derive(Serialize)]
struct SarifInvocation {
    #[serde(rename = "executionSuccessful")]
    execution_successful: bool,
}

#[derive(Serialize)]
struct SarifRunProperties {
    #[serde(rename = "reviewText")]
    review_text: String,
    stats: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{ReviewFinding, ReviewStats};

    fn sample_result() -> ReviewResult {
        ReviewResult {
            review_text: "## Review\n\nLooks OK.".into(),
            findings: vec![
                ReviewFinding {
                    severity: "high".into(),
                    file: Some("src/main.rs".into()),
                    line: Some(42),
                    category: "security".into(),
                    message: "Hardcoded password".into(),
                    suggestion: Some("Use env var".into()),
                },
                ReviewFinding {
                    severity: "low".into(),
                    file: None,
                    line: None,
                    category: "best_practice".into(),
                    message: "Use const".into(),
                    suggestion: None,
                },
            ],
            pr_number: Some(1),
            pr_title: Some("Test".into()),
            session_id: None,
            stats: ReviewStats {
                files_changed: 2,
                files_reviewed: 1,
                files_skipped: 0,
                files_path_filtered: 0,
                files_budget_dropped: 0,
                input_tokens_estimated: 100,
                system_tokens_estimated: 200,
                output_tokens_reported: Some(50),
                total_tokens_used: Some(350),
                latency_ms: 500,
                model: "test-model".into(),
                prompt_version: "code/1".into(),
                domain: "code".into(),
            },
        }
    }

    #[test]
    fn test_sarif_has_schema() {
        let result = sample_result();
        let sarif = to_sarif_value(&result);
        assert_eq!(
            sarif["$schema"].as_str().unwrap(),
            "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json"
        );
        assert_eq!(sarif["version"], "2.1.0");
    }

    #[test]
    fn test_sarif_has_two_results() {
        let result = sample_result();
        let sarif = to_sarif_value(&result);
        let results = sarif["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_sarif_severity_mapping() {
        let result = sample_result();
        let sarif = to_sarif_value(&result);
        let results = sarif["runs"][0]["results"].as_array().unwrap();
        // high → error
        assert_eq!(results[0]["level"], "error");
        // low → note
        assert_eq!(results[1]["level"], "note");
    }

    #[test]
    fn test_sarif_rule_index() {
        let result = sample_result();
        let sarif = to_sarif_value(&result);
        let rules = sarif["runs"][0]["tool"]["driver"]["rules"]
            .as_array()
            .unwrap();
        // BTreeMap orders alphabetically: best_practice before security
        assert_eq!(rules[0]["id"], "best_practice");
        assert_eq!(rules[1]["id"], "security");
    }

    #[test]
    fn test_sarif_location() {
        let result = sample_result();
        let sarif = to_sarif_value(&result);
        let results = sarif["runs"][0]["results"].as_array().unwrap();
        let locs = results[0]["locations"].as_array().unwrap();
        assert_eq!(
            locs[0]["physicalLocation"]["artifactLocation"]["uri"],
            "src/main.rs"
        );
        assert_eq!(locs[0]["physicalLocation"]["region"]["startLine"], 42);
    }

    #[test]
    fn test_sarif_fix() {
        let result = sample_result();
        let sarif = to_sarif_value(&result);
        let results = sarif["runs"][0]["results"].as_array().unwrap();
        let fixes = results[0]["fixes"].as_array().unwrap();
        assert_eq!(fixes[0]["description"]["text"], "Use env var");
    }

    #[test]
    fn test_sarif_no_location_for_project_wide() {
        let result = sample_result();
        let sarif = to_sarif_value(&result);
        let results = sarif["runs"][0]["results"].as_array().unwrap();
        // Second finding has no file/line — locations should be empty
        assert!(results[1]["locations"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_sarif_invocation_successful() {
        let result = sample_result();
        let sarif = to_sarif_value(&result);
        assert!(
            sarif["runs"][0]["invocations"][0]["executionSuccessful"]
                .as_bool()
                .unwrap()
        );
    }

    #[test]
    fn test_sarif_properties_include_stats() {
        let result = sample_result();
        let sarif = to_sarif_value(&result);
        let props = &sarif["runs"][0]["properties"];
        assert!(props["stats"]["model"].is_string());
        assert_eq!(props["reviewText"], "## Review\n\nLooks OK.");
    }
}
