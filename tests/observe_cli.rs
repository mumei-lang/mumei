use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new(source: &str, harvest: &Value) -> Self {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mumei-observe-{id}"));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("input.mm"), source).unwrap();
        fs::write(
            path.join("harvest.json"),
            serde_json::to_vec(harvest).unwrap(),
        )
        .unwrap();
        Self(path)
    }

    fn input(&self) -> PathBuf {
        self.0.join("input.mm")
    }

    fn harvest(&self) -> PathBuf {
        self.0.join("harvest.json")
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .arg("observe")
            .arg(self.input())
            .arg(self.harvest())
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn harvest(observations: Vec<Value>) -> Value {
    json!({"schema": "mumei.test-harvest/v1", "observations": observations})
}

fn observation(kind: &str, target: &str, args: Value) -> Value {
    json!({
        "kind": kind,
        "target": target,
        "test": "test_observed",
        "file": "test_module.py",
        "line": 4,
        "args": args
    })
}

fn example(target: &str, args: Value, result: Value) -> Value {
    let mut item = observation("example", target, args);
    item["result"] = result;
    item
}

fn parse(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid observe JSON: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[test]
fn examples_and_refinement_inputs_are_classified() {
    let fixture = Fixture::new(
        "type Nat = i64 where v >= 0;\n\
         atom add(x: i64) -> i64\nrequires: x >= 0;\nensures: result == x + 1;\nbody: x + 1;\n\
         atom nat_id(x: Nat) -> i64\nrequires: true;\nensures: result == x;\nbody: x;\n",
        &harvest(vec![
            {
                let mut item = observation("example", "add", json!({"x": 2}));
                item["result"] = json!(3);
                item
            },
            {
                let mut item = observation("example", "add", json!({"x": 2}));
                item["result"] = json!(9);
                item
            },
            {
                let mut item = observation("example", "add", json!({"x": -1}));
                item["result"] = json!(0);
                item
            },
            {
                let mut item = observation("example", "nat_id", json!({"x": -1}));
                item["result"] = json!(-1);
                item
            },
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(output.status.code(), Some(1));
    let report = parse(&output);
    let results = report["results"].as_array().unwrap();
    assert_eq!(results[0]["verdict"], "consistent");
    assert_eq!(results[1]["verdict"], "contradicts_ensures");
    assert_eq!(
        results[2]["verdict"],
        "outside_requires",
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(results[3]["verdict"], "outside_requires");
    assert_eq!(report["totals"]["consistent"], 1);
    assert_eq!(report["totals"]["contradicts_ensures"], 1);
}

#[test]
fn relations_rejects_and_positional_arguments_are_classified() {
    let fixture = Fixture::new(
        "atom f(x: i64) -> i64\nrequires: x >= 0;\nensures: result == x + 1;\nbody: x + 1;\n",
        &harvest(vec![
            {
                let mut item = observation("relation", "f", json!([2]));
                item["predicate"] = json!("result > 0");
                item
            },
            {
                let mut item = observation("relation", "f", json!({"x": 2}));
                item["predicate"] = json!("result == 0");
                item
            },
            observation("rejects", "f", json!({"x": -1})),
            observation("rejects", "f", json!({"x": 1})),
            {
                let mut item = observation("example", "f", json!([]));
                item["result"] = json!(0);
                item
            },
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(output.status.code(), Some(1));
    let results = parse(&output)["results"].as_array().unwrap().clone();
    assert_eq!(results[0]["verdict"], "consistent");
    assert_eq!(results[1]["verdict"], "contradicts_ensures");
    assert_eq!(
        results[2]["verdict"],
        "rejection_explained",
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(results[3]["verdict"], "requires_admits_rejected");
    assert_eq!(results[4]["verdict"], "not_checked");
    assert_eq!(results[4]["detail"], "argument_count_mismatch");
}

#[test]
fn unmatched_and_unsupported_observations_are_not_checked() {
    let fixture = Fixture::new(
        "trusted atom trusted_id(x: i64) -> i64\nrequires: true;\nensures: result == x;\nbody: 0;\n\
         atom identity<T>(x: T) -> T\nrequires: true;\nensures: result == x;\nbody: x;\n\
         atom flag(x: bool) -> bool\nrequires: true;\nensures: result == x;\nbody: x;\n",
        &harvest(vec![
            {
                let mut item = observation("example", "trusted_id", json!({"x": 2}));
                item["result"] = json!(2);
                item
            },
            {
                let mut item = observation("example", "missing", json!({"x": 2}));
                item["result"] = json!(2);
                item
            },
            {
                let mut item = observation("example", "identity", json!({"x": 2}));
                item["result"] = json!(2);
                item
            },
            {
                let mut item = observation("example", "flag", json!({"wrong": true}));
                item["result"] = json!(true);
                item
            },
            {
                let mut item = observation("example", "flag", json!({"x": 1}));
                item["result"] = json!(true);
                item
            },
            {
                let mut item = observation("example", "trusted_id", json!({"x": true}));
                item["result"] = json!(1);
                item
            },
            {
                let mut item = observation("example", "trusted_id", json!({"x": "not an int"}));
                item["result"] = json!(1);
                item
            },
            {
                let mut item = observation("example", "flag", json!({"x": true, "extra": 1}));
                item["result"] = json!(true);
                item
            },
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(output.status.code(), Some(0));
    let results = parse(&output)["results"].as_array().unwrap().clone();
    assert_eq!(results[0]["verdict"], "consistent");
    assert_eq!(results[1]["detail"], "no_atom");
    assert_eq!(results[2]["detail"], "generic_atom");
    assert_eq!(results[3]["detail"], "argument_names_mismatch");
    assert_eq!(results[4]["detail"], "int_for_bool_type");
    assert_eq!(results[5]["detail"], "bool_for_non_bool_type");
    assert_eq!(results[6]["detail"], "unsupported_value_type");
    assert_eq!(results[7]["detail"], "argument_names_mismatch");
}

#[test]
fn bool_negative_minimum_multi_param_and_atom_filter_work() {
    let fixture = Fixture::new(
        "atom pair(x: i64, y: i64) -> i64\nrequires: true;\nensures: result == x + y;\nbody: x + y;\n\
         atom identity(x: i64) -> i64\nrequires: true;\nensures: result == x;\nbody: x;\n\
         atom flip(x: bool) -> bool\nrequires: true;\nensures: result == !x;\nbody: !x;\n",
        &harvest(vec![
            {
                let mut item = observation("example", "pair", json!({"x": -2, "y": 1}));
                item["result"] = json!(-1);
                item
            },
            {
                let mut item = observation("example", "identity", json!({"x": i64::MIN}));
                item["result"] = json!(i64::MIN);
                item
            },
            {
                let mut item = observation("example", "identity", json!({"x": i64::MAX}));
                item["result"] = json!(i64::MAX);
                item
            },
            {
                let mut item = observation("example", "identity", json!({"x": 0}));
                item["args"]["x"] =
                    serde_json::from_str("9223372036854775808").expect("i64 max + 1");
                item["result"] = json!(0);
                item
            },
            {
                let mut item = observation("example", "identity", json!({"x": 0}));
                item["args"]["x"] = serde_json::from_str(
                    "1000000000000000000000000000000000000000000000000000000000000000000",
                )
                .expect("large integer");
                item["result"] = json!(0);
                item
            },
            {
                let mut item = observation("example", "identity", json!({"x": 0}));
                item["args"]["x"] =
                    serde_json::from_str("-9223372036854775809").expect("i64 min - 1");
                item["result"] = json!(0);
                item
            },
            {
                let mut item = observation("example", "flip", json!({"x": true}));
                item["result"] = json!(false);
                item
            },
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(output.status.code(), Some(0));
    let report = parse(&output);
    let results = report["results"].as_array().unwrap();
    assert_eq!(results[0]["verdict"], "consistent");
    assert_eq!(results[1]["verdict"], "consistent");
    assert_eq!(results[2]["verdict"], "consistent");
    assert_eq!(results[3]["detail"], "integer_out_of_i64_range");
    assert_eq!(results[4]["detail"], "integer_out_of_i64_range");
    assert_eq!(results[5]["detail"], "integer_out_of_i64_range");
    assert_eq!(results[6]["verdict"], "consistent");

    let filtered = fixture.run(&["--json", "--atom", "identity"]);
    assert_eq!(filtered.status.code(), Some(0));
    let report = parse(&filtered);
    assert_eq!(report["results"].as_array().unwrap().len(), 5);
    assert!(report["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|result| result["target"] == "identity"));
}

#[test]
fn ensures_may_call_another_atom() {
    let fixture = Fixture::new(
        "atom helper(x: i64) -> i64\nrequires: true;\nensures: result == x + 1;\nbody: x + 1;\n\
         atom f(x: i64) -> i64\nrequires: true;\nensures: result == helper(x);\nbody: helper(x);\n",
        &harvest(vec![{
            let mut item = observation("example", "f", json!({"x": 3}));
            item["result"] = json!(4);
            item
        }]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(parse(&output)["results"][0]["verdict"], "consistent");
}

#[test]
fn invalid_inputs_exit_four() {
    let fixture = Fixture::new(
        "atom id(x: i64) -> i64\nrequires: true;\nensures: result == x;\nbody: x;\n",
        &harvest(vec![]),
    );
    assert_eq!(fixture.run(&["--json"]).status.code(), Some(0));
    fs::write(fixture.harvest(), "{").unwrap();
    assert_eq!(fixture.run(&["--json"]).status.code(), Some(4));
    fs::write(
        fixture.harvest(),
        r#"{"schema":"other/v1","observations":[]}"#,
    )
    .unwrap();
    assert_eq!(fixture.run(&["--json"]).status.code(), Some(4));
    fs::write(fixture.harvest(), r#"{"schema":"mumei.test-harvest/v1"}"#).unwrap();
    assert_eq!(fixture.run(&["--json"]).status.code(), Some(4));

    fs::write(fixture.input(), "atom broken(").unwrap();
    fs::write(
        fixture.harvest(),
        r#"{"schema":"mumei.test-harvest/v1","observations":[]}"#,
    )
    .unwrap();
    assert_eq!(fixture.run(&["--json"]).status.code(), Some(4));
}

#[test]
fn output_is_deterministic_and_wrappers_leave_cwd_clean() {
    let fixture = Fixture::new(
        "atom id(x: i64) -> i64\nrequires: true;\nensures: result == x;\nbody: x;\n",
        &harvest(vec![{
            let mut item = observation("example", "id", json!({"x": 7}));
            item["result"] = json!(7);
            item
        }]),
    );
    let first = fixture.run(&["--json"]);
    let second = fixture.run(&["--json"]);
    assert_eq!(first.status.code(), Some(0));
    assert_eq!(first.stdout, second.stdout);
    let names = fs::read_dir(&fixture.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 2);
    assert!(names.iter().any(|name| name == "input.mm"));
    assert!(names.iter().any(|name| name == "harvest.json"));
}

#[test]
fn human_output_is_available_without_json_flag() {
    let fixture = Fixture::new(
        "atom id(x: i64) -> i64\nrequires: true;\nensures: result == x;\nbody: x;\n",
        &harvest(vec![{
            let mut item = observation("example", "id", json!({"x": 2}));
            item["result"] = json!(2);
            item
        }]),
    );
    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("consistent id"));
}

#[test]
fn clause_modes_and_labels_are_preserved_in_observation_checks() {
    let fixture = Fixture::new(
        "atom assumed(x: i64) -> i64\n\
         requires assume: x > 0;\n\
         ensures: result == x;\n\
         body: x;\n\
         atom checked(x: i64) -> i64\n\
         requires: x > 0;\n\
         ensures check: result == x;\n\
         body: x;\n\
         atom labelled(x: i64) -> i64\n\
         requires \"positive input\": x > 0;\n\
         ensures \"identity result\": result == x;\n\
         body: x;\n",
        &harvest(vec![
            example("assumed", json!({"x": 2}), json!(2)),
            example("assumed", json!({"x": 2}), json!(3)),
            example("assumed", json!({"x": 0}), json!(0)),
            example("checked", json!({"x": 2}), json!(2)),
            example("checked", json!({"x": 2}), json!(3)),
            example("labelled", json!({"x": 2}), json!(2)),
            example("labelled", json!({"x": 2}), json!(3)),
            example("labelled", json!({"x": 0}), json!(0)),
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(output.status.code(), Some(1));
    let results = parse(&output)["results"].as_array().unwrap().clone();
    assert_eq!(results[0]["verdict"], "consistent");
    assert_eq!(results[1]["verdict"], "contradicts_ensures");
    assert_eq!(results[2]["verdict"], "outside_requires");
    assert_eq!(results[3]["verdict"], "consistent");
    assert_eq!(results[4]["verdict"], "contradicts_ensures");
    assert_eq!(results[5]["verdict"], "consistent");
    assert_eq!(results[6]["verdict"], "contradicts_ensures");
    assert_eq!(results[7]["verdict"], "outside_requires");
}

#[test]
fn structured_parameter_values_are_not_checked() {
    let fixture = Fixture::new(
        "struct Point { x: i64 }\n\
         atom point(p: Point) -> i64\n\
         requires: true;\n\
         ensures: result == 0;\n\
         body: 0;\n\
         atom total(values: [i64]) -> i64\n\
         requires: true;\n\
         ensures: result == 0;\n\
         body: 0;\n",
        &harvest(vec![
            example("point", json!({"p": {"x": 7}}), json!(7)),
            example("total", json!({"values": [1, 2]}), json!(3)),
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(output.status.code(), Some(0));
    let results = parse(&output)["results"].as_array().unwrap().clone();
    assert_eq!(results.len(), 2);
    for result in results {
        assert_eq!(result["verdict"], "not_checked");
        assert_eq!(result["detail"], "unsupported_value_type");
    }
}

#[test]
fn bitvec_semantics_are_preserved_in_observation_wrappers() {
    let fixture = Fixture::new(
        "atom wrapping(x: i64) -> i64\n\
         semantics: bitvec;\n\
         requires: true;\n\
         ensures: result == x + 1;\n\
         body: x + 1;\n",
        &harvest(vec![
            example("wrapping", json!({"x": i64::MAX}), json!(i64::MIN)),
            example("wrapping", json!({"x": i64::MAX}), json!(0)),
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(output.status.code(), Some(1));
    let results = parse(&output)["results"].as_array().unwrap().clone();
    assert_eq!(results[0]["verdict"], "consistent");
    assert_eq!(results[1]["verdict"], "contradicts_ensures");
}
