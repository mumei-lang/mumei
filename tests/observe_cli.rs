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

fn property(target: &str, args: Value, variables: Value, predicate: &str) -> Value {
    let mut item = observation("property", target, args);
    item["variables"] = variables;
    item["assumptions"] = json!([]);
    item["predicate"] = json!(predicate);
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
    let report = parse(&output);
    let results = report["results"].as_array().unwrap().clone();
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
    for verdict in ["entailed", "not_entailed", "domain_exceeds_requires"] {
        assert_eq!(report["totals"][verdict], 0);
    }
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
        &harvest(vec![
            {
                let mut item = observation("example", "id", json!({"x": 7}));
                item["result"] = json!(7);
                item
            },
            property(
                "id",
                json!({"x": {"var": "x"}}),
                json!([{"name": "x", "type": "i64", "min": 0, "max": 10}]),
                "result == x",
            ),
        ]),
    );
    let first = fixture.run(&["--json"]);
    let second = fixture.run(&["--json"]);
    assert_eq!(first.status.code(), Some(0));
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(parse(&first)["results"][1]["verdict"], "entailed");
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
        &harvest(vec![
            {
                let mut item = observation("example", "id", json!({"x": 2}));
                item["result"] = json!(2);
                item
            },
            property(
                "id",
                json!({"x": {"var": "x"}}),
                json!([{"name": "x", "type": "i64"}]),
                "result == x",
            ),
        ]),
    );
    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("consistent id"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("entailed id"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("entailed 1"));
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

#[test]
fn property_observations_are_entailed_or_not_entailed() {
    let fixture = Fixture::new(
        "atom add(a: i64, b: i64) -> i64\n\
         requires: true;\n\
         ensures: result == a + b;\n\
         body: a + b;\n",
        &harvest(vec![
            property(
                "add",
                json!({"a": {"var": "x"}, "b": {"var": "y"}}),
                json!([
                    {"name": "x", "type": "i64", "min": 0, "max": 10},
                    {"name": "y", "type": "i64", "min": 0, "max": 10}
                ]),
                "result == x + y",
            ),
            property(
                "add",
                json!({"a": {"var": "x"}, "b": {"var": "y"}}),
                json!([
                    {"name": "x", "type": "i64", "min": 0, "max": 10},
                    {"name": "y", "type": "i64", "min": 0, "max": 10}
                ]),
                "result == x",
            ),
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse(&output);
    assert_eq!(report["results"][0]["verdict"], "entailed");
    assert_eq!(report["results"][1]["verdict"], "not_entailed");
    assert_eq!(report["totals"]["entailed"], 1);
    assert_eq!(report["totals"]["not_entailed"], 1);
}

#[test]
fn property_reserved_generator_names_are_not_checked() {
    let fixture = Fixture::new(
        "atom id(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
        &harvest(vec![
            property(
                "id",
                json!({"x": {"var": "result"}}),
                json!([{"name": "result", "type": "i64"}]),
                "result == result",
            ),
            property(
                "id",
                json!({"x": {"var": "true"}}),
                json!([{"name": "true", "type": "bool"}]),
                "result == true",
            ),
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let results = parse(&output)["results"].as_array().unwrap().clone();
    assert_eq!(results[0]["verdict"], "not_checked");
    assert_eq!(results[0]["detail"], "reserved_variable_name");
    assert_eq!(results[1]["verdict"], "not_checked");
    assert_eq!(results[1]["detail"], "reserved_variable_name");
}

#[test]
fn property_generator_renaming_is_single_pass_for_overlapping_names() {
    let fixture = Fixture::new(
        "atom add(a: i64, b: i64) -> i64\n\
         requires: true;\n\
         ensures: result == a + b;\n\
         body: a + b;\n",
        &harvest(vec![
            {
                let mut item = property(
                    "add",
                    json!({"a": {"var": "x"}, "b": {"var": "__gen_x"}}),
                    json!([
                        {"name": "x", "type": "i64", "min": 0, "max": 10},
                        {"name": "__gen_x", "type": "i64", "min": 0, "max": 10}
                    ]),
                    "result == x + __gen_x",
                );
                item["assumptions"] = json!(["x >= 0", "__gen_x >= 0"]);
                item
            },
            property(
                "add",
                json!({"a": {"var": "x"}, "b": {"var": "__gen_x"}}),
                json!([
                    {"name": "x", "type": "i64", "min": 0, "max": 10},
                    {"name": "__gen_x", "type": "i64", "min": 0, "max": 10}
                ]),
                "result == x",
            ),
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let results = parse(&output)["results"].as_array().unwrap().clone();
    assert_eq!(results[0]["verdict"], "entailed");
    assert_eq!(results[1]["verdict"], "not_entailed");
}

#[test]
fn property_generator_prefix_avoids_atom_parameter_collisions() {
    let fixture = Fixture::new(
        "atom id(__gen_x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == __gen_x;\n\
         body: __gen_x;\n\
         atom add(__gen_x: i64, __gen1_x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == __gen_x + __gen1_x;\n\
         body: __gen_x + __gen1_x;\n",
        &harvest(vec![
            property(
                "id",
                json!({"__gen_x": {"var": "x"}}),
                json!([{"name": "x", "type": "i64", "min": 0, "max": 10}]),
                "result == x",
            ),
            property(
                "add",
                json!({"__gen_x": {"var": "x"}, "__gen1_x": 2}),
                json!([{"name": "x", "type": "i64", "min": 0, "max": 10}]),
                "result == x + 2",
            ),
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let results = parse(&output)["results"].as_array().unwrap().clone();
    assert_eq!(results[0]["verdict"], "entailed");
    assert_eq!(results[1]["verdict"], "entailed");
}

#[test]
fn property_contradictions_exit_one() {
    let fixture = Fixture::new(
        "atom successor(a: i64) -> i64\n\
         requires: true;\n\
         ensures: result == a + 1;\n\
         body: a + 1;\n",
        &harvest(vec![property(
            "successor",
            json!({"a": {"var": "x"}}),
            json!([{"name": "x", "type": "i64"}]),
            "result == x",
        )]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        parse(&output)["results"][0]["verdict"],
        "contradicts_ensures"
    );
}

#[test]
fn property_domains_check_requires_and_assumptions() {
    let fixture = Fixture::new(
        "atom nonnegative(a: i64) -> i64\n\
         requires: a >= 0;\n\
         ensures: result == a;\n\
         body: a;\n\
         atom high(a: i64) -> i64\n\
         requires: a >= 100;\n\
         ensures: result == a;\n\
         body: a;\n\
         atom no_five(a: i64) -> i64\n\
         requires: a != 5;\n\
         ensures: result == a;\n\
         body: a;\n",
        &harvest(vec![
            property(
                "nonnegative",
                json!({"a": {"var": "x"}}),
                json!([{"name": "x", "type": "i64", "min": -5, "max": 5}]),
                "result == x",
            ),
            property(
                "high",
                json!({"a": {"var": "x"}}),
                json!([{"name": "x", "type": "i64", "min": 0, "max": 10}]),
                "result == x",
            ),
            {
                let mut item = property(
                    "no_five",
                    json!({"a": {"var": "x"}}),
                    json!([{"name": "x", "type": "i64", "min": 0, "max": 10}]),
                    "result == x",
                );
                item["assumptions"] = json!(["x != 5"]);
                item
            },
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let results = parse(&output)["results"].as_array().unwrap().clone();
    assert_eq!(results[0]["verdict"], "domain_exceeds_requires");
    assert_eq!(results[1]["verdict"], "outside_requires");
    assert_eq!(results[2]["verdict"], "entailed");
}

#[test]
fn property_arguments_support_same_named_variables_literals_and_positions() {
    let fixture = Fixture::new(
        "atom add(a: i64, b: i64) -> i64\n\
         requires: true;\n\
         ensures: result == a + b;\n\
         body: a + b;\n",
        &harvest(vec![
            property(
                "add",
                json!({"a": {"var": "a"}, "b": 3}),
                json!([{"name": "a", "type": "i64", "min": -10, "max": 10}]),
                "result == a + 3",
            ),
            property(
                "add",
                json!([{"var": "y"}, 3]),
                json!([{"name": "y", "type": "i64", "min": -10}]),
                "result == y + 3",
            ),
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let results = parse(&output)["results"].as_array().unwrap().clone();
    assert_eq!(results[0]["verdict"], "entailed");
    assert_eq!(results[1]["verdict"], "entailed");
}

#[test]
fn property_bool_unbounded_half_bounded_and_extreme_domains_work() {
    let fixture = Fixture::new(
        "atom id(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n\
         atom invert(enabled: bool) -> bool\n\
         requires: true;\n\
         ensures: result == !enabled;\n\
         body: !enabled;\n",
        &harvest(vec![
            property(
                "id",
                json!({"x": {"var": "free"}}),
                json!([{"name": "free", "type": "i64"}]),
                "result == free",
            ),
            property(
                "id",
                json!({"x": {"var": "minimum"}}),
                json!([{"name": "minimum", "type": "i64", "min": -5}]),
                "result == minimum",
            ),
            property(
                "id",
                json!({"x": {"var": "maximum"}}),
                json!([{"name": "maximum", "type": "i64", "max": 5}]),
                "result == maximum",
            ),
            property(
                "id",
                json!({"x": {"var": "extreme"}}),
                json!([{
                    "name": "extreme",
                    "type": "i64",
                    "min": i64::MIN,
                    "max": i64::MAX
                }]),
                "result == extreme",
            ),
            property(
                "invert",
                json!({"enabled": {"var": "flag"}}),
                json!([{"name": "flag", "type": "bool"}]),
                "result == !flag",
            ),
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(parse(&output)["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|result| result["verdict"] == "entailed"));
}

#[test]
fn property_argument_mismatches_and_unknown_identifiers_fail_closed() {
    let fixture = Fixture::new(
        "atom id(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
        &harvest(vec![
            property(
                "id",
                json!({"x": {"var": "flag"}}),
                json!([{"name": "flag", "type": "bool"}]),
                "result == flag",
            ),
            property(
                "id",
                json!({"x": {"var": "missing"}}),
                json!([{"name": "known", "type": "i64"}]),
                "result == known",
            ),
            property(
                "id",
                json!({"x": {"var": "known"}}),
                json!([{"name": "known", "type": "i64"}]),
                "result == known + unknown",
            ),
        ]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(output.status.code(), Some(0));
    let results = parse(&output)["results"].as_array().unwrap().clone();
    assert_eq!(results[0]["verdict"], "not_checked");
    assert_eq!(results[0]["detail"], "argument_type_mismatch");
    assert_eq!(results[1]["verdict"], "not_checked");
    assert_eq!(results[1]["detail"], "unknown_variable");
    assert_eq!(results[2]["verdict"], "not_checked");
    assert_eq!(results[2]["detail"], "unknown_identifier");
    assert_ne!(results[2]["verdict"], "entailed");
    assert_ne!(results[2]["verdict"], "consistent");
}

#[test]
fn malformed_property_structures_exit_four() {
    let mut malformed = vec![
        property(
            "id",
            json!({"x": 1}),
            json!({"name": "x", "type": "i64"}),
            "result == x",
        ),
        property(
            "id",
            json!({"x": 1}),
            json!([{"name": "x", "type": "f64"}]),
            "result == x",
        ),
        property(
            "id",
            json!({"x": 1}),
            json!([{"name": "x", "type": "i64", "min": 2, "max": 1}]),
            "result == x",
        ),
        property(
            "id",
            json!({"x": 1}),
            json!([{"name": "x", "type": "i64", "min": "zero"}]),
            "result == x",
        ),
    ];
    let mut missing_predicate = property(
        "id",
        json!({"x": 1}),
        json!([{"name": "x", "type": "i64"}]),
        "result == x",
    );
    missing_predicate
        .as_object_mut()
        .unwrap()
        .remove("predicate");
    malformed.push(missing_predicate);

    for item in malformed {
        let fixture = Fixture::new(
            "atom id(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result == x;\n\
             body: x;\n",
            &harvest(vec![item]),
        );
        let output = fixture.run(&["--json"]);
        assert_eq!(
            output.status.code(),
            Some(4),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn property_atom_filter_and_unsupported_atom_kinds_work() {
    let fixture = Fixture::new(
        "type Nat = i64 where v >= 0;\n\
         trusted atom trusted_id(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n\
         atom nat_id(x: Nat) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n\
         atom id(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
        &harvest(vec![
            property(
                "trusted_id",
                json!({"x": {"var": "x"}}),
                json!([{"name": "x", "type": "i64"}]),
                "result == x",
            ),
            property(
                "nat_id",
                json!({"x": {"var": "x"}}),
                json!([{"name": "x", "type": "i64"}]),
                "result == x",
            ),
            property(
                "missing",
                json!({"x": {"var": "x"}}),
                json!([{"name": "x", "type": "i64"}]),
                "result == x",
            ),
            property(
                "id",
                json!({"x": {"var": "x"}}),
                json!([{"name": "x", "type": "i64"}]),
                "result == x",
            ),
        ]),
    );
    let output = fixture.run(&["--json", "--atom", "id"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse(&output);
    let results = report["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["verdict"], "entailed");

    let output = fixture.run(&["--json"]);
    assert_eq!(output.status.code(), Some(0));
    let results = parse(&output)["results"].as_array().unwrap().clone();
    assert_eq!(results[0]["detail"], "trusted_atom");
    assert_eq!(results[1]["detail"], "refinement_parameter");
    assert_eq!(results[2]["detail"], "no_atom");
    assert_eq!(results[3]["verdict"], "entailed");
}

#[test]
fn property_checks_preserve_bitvec_semantics() {
    let fixture = Fixture::new(
        "atom wrapping(a: i64) -> i64\n\
         semantics: bitvec;\n\
         requires: true;\n\
         ensures: result == a + 1;\n\
         body: a + 1;\n",
        &harvest(vec![property(
            "wrapping",
            json!({"a": {"var": "x"}}),
            json!([{"name": "x", "type": "i64", "min": i64::MAX, "max": i64::MAX}]),
            "result == x + 1",
        )]),
    );
    let output = fixture.run(&["--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(parse(&output)["results"][0]["verdict"], "entailed");
}
