use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use std::collections::HashMap;
use z3::ast::Dynamic;
use z3::Model;

pub const RECONSTRUCTION_LOSS_SCHEMA_VERSION: &str = "p9-de/v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormalSpace {
    pub symbol: String,
    pub definition: String,
}

impl Default for FormalSpace {
    fn default() -> Self {
        Self {
            symbol: "∅".to_string(),
            definition: "No formal space was recorded.".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LossComponent {
    pub variable: String,
    pub observed: Value,
    pub magnitude: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReconstructionLossFormalization {
    pub specification_space: FormalSpace,
    pub implementation_space: FormalSpace,
    pub metric: String,
    pub zero_loss_condition: String,
}

impl Default for ReconstructionLossFormalization {
    fn default() -> Self {
        Self {
            specification_space: FormalSpace {
                symbol: "S".to_string(),
                definition: "The contract/specification space induced by requires/ensures clauses."
                    .to_string(),
            },
            implementation_space: FormalSpace {
                symbol: "V".to_string(),
                definition: "The verified implementation space induced by the atom body and path constraints."
                    .to_string(),
            },
            metric: "L_recon(S,V,c) = ||eval_S(c) - eval_V(c)|| over the Z3 counterexample c."
                .to_string(),
            zero_loss_condition:
                "L_recon = 0 iff no counterexample component has non-zero magnitude."
                    .to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReconstructionLoss {
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    #[serde(default)]
    pub formalization: ReconstructionLossFormalization,
    pub violated_property: String,
    pub counter_example: HashMap<String, Value>,
    pub loss_vector: Vec<f32>,
    #[serde(default)]
    pub loss_components: Vec<LossComponent>,
}

impl ReconstructionLoss {
    pub fn from_z3_model(
        violated_property: impl Into<String>,
        model: &Model<'_>,
        variables: &HashMap<String, Dynamic<'_>>,
    ) -> Self {
        let mut counter_example = HashMap::new();
        for (name, variable) in variables {
            if let Some(value) = model.eval(variable, true) {
                counter_example.insert(name.clone(), dynamic_to_json(&value));
            }
        }
        Self::from_counter_example(violated_property, counter_example)
    }

    pub fn from_counter_example(
        violated_property: impl Into<String>,
        counter_example: HashMap<String, Value>,
    ) -> Self {
        let mut keys: Vec<&String> = counter_example.keys().collect();
        keys.sort();
        let loss_components = keys
            .into_iter()
            .filter_map(|key| {
                counter_example.get(key).and_then(|value| {
                    value_to_loss_component(value).map(|magnitude| LossComponent {
                        variable: key.clone(),
                        observed: value.clone(),
                        magnitude,
                    })
                })
            })
            .collect::<Vec<_>>();
        let loss_vector = loss_components
            .iter()
            .map(|component| component.magnitude)
            .collect();
        Self {
            schema_version: default_schema_version(),
            formalization: ReconstructionLossFormalization::default(),
            violated_property: violated_property.into(),
            counter_example,
            loss_vector,
            loss_components,
        }
    }

    pub fn from_counterexample_value(
        violated_property: impl Into<String>,
        counterexample: &Value,
    ) -> Option<Self> {
        let object = counterexample.as_object()?;
        let counter_example = object
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Some(Self::from_counter_example(
            violated_property,
            counter_example,
        ))
    }

    pub fn is_zero_loss(&self) -> bool {
        self.loss_vector.is_empty()
            || self
                .loss_vector
                .iter()
                .all(|component| component.abs() <= f32::EPSILON)
    }

    pub fn total_magnitude(&self) -> f32 {
        self.loss_vector
            .iter()
            .map(|component| component.abs())
            .sum()
    }

    pub fn verifies_zero_loss(&self) -> bool {
        self.total_magnitude() <= f32::EPSILON
            && self
                .loss_components
                .iter()
                .all(|component| component.magnitude.abs() <= f32::EPSILON)
    }
}

fn default_schema_version() -> String {
    RECONSTRUCTION_LOSS_SCHEMA_VERSION.to_string()
}

fn dynamic_to_json(value: &Dynamic<'_>) -> Value {
    if let Some(int_value) = value.as_int().and_then(|int_value| int_value.as_i64()) {
        return Value::Number(Number::from(int_value));
    }
    if let Some((num, den)) = value.as_real().and_then(|real_value| real_value.as_real()) {
        if den != 0 {
            if let Some(number) = Number::from_f64(num as f64 / den as f64) {
                return Value::Number(number);
            }
        }
    }
    if let Some(bool_value) = value.as_bool().and_then(|bool_value| bool_value.as_bool()) {
        return Value::Bool(bool_value);
    }
    if let Some(string_value) = value
        .as_string()
        .and_then(|string_value| string_value.as_string())
    {
        return Value::String(string_value);
    }
    Value::String(value.to_string())
}

fn value_to_loss_component(value: &Value) -> Option<f32> {
    match value {
        Value::Number(number) => number.as_f64().map(|value| value.abs() as f32),
        Value::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        Value::String(value) => value
            .parse::<f32>()
            .ok()
            .map(f32::abs)
            .or_else(|| (!value.is_empty()).then_some(1.0)),
        Value::Array(values) => {
            let sum: f32 = values.iter().filter_map(value_to_loss_component).sum();
            Some(sum)
        }
        Value::Object(values) => {
            let sum: f32 = values.values().filter_map(value_to_loss_component).sum();
            Some(sum)
        }
        Value::Null => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use z3::ast::{Ast, Int};
    use z3::{Config, Context, SatResult, Solver};

    #[test]
    fn test_from_z3_model_records_counterexample_and_loss_vector() {
        let mut config = Config::new();
        config.set_model_generation(true);
        let context = Context::new(&config);
        let solver = Solver::new(&context);
        let x = Int::new_const(&context, "x");
        solver.assert(&x._eq(&Int::from_i64(&context, 7)));

        assert_eq!(solver.check(), SatResult::Sat);
        let model = solver.get_model().expect("sat model");
        let variables = HashMap::from([("x".to_string(), Dynamic::from(&x))]);

        let loss = ReconstructionLoss::from_z3_model("result > 10", &model, &variables);

        assert_eq!(loss.violated_property, "result > 10");
        assert_eq!(loss.schema_version, RECONSTRUCTION_LOSS_SCHEMA_VERSION);
        assert_eq!(
            loss.counter_example.get("x"),
            Some(&Value::Number(7.into()))
        );
        assert_eq!(loss.loss_vector, vec![7.0]);
        assert_eq!(loss.loss_components[0].variable, "x");
        assert_eq!(loss.loss_components[0].magnitude, 7.0);
        assert!(!loss.is_zero_loss());
        assert_eq!(loss.total_magnitude(), 7.0);
    }

    #[test]
    fn test_zero_loss_detection() {
        let empty = ReconstructionLoss::from_counter_example("result == 0", HashMap::new());
        assert!(empty.is_zero_loss());

        let zero = ReconstructionLoss::from_counter_example(
            "result == 0",
            HashMap::from([("x".to_string(), Value::Number(0.into()))]),
        );
        assert!(zero.is_zero_loss());
        assert!(zero.verifies_zero_loss());
    }

    #[test]
    fn test_deserializes_legacy_loss_payload() {
        let legacy = serde_json::json!({
            "violated_property": "result > 0",
            "counter_example": {"x": -1},
            "loss_vector": [1.0]
        });

        let loss: ReconstructionLoss = serde_json::from_value(legacy).expect("legacy loss");

        assert_eq!(loss.schema_version, RECONSTRUCTION_LOSS_SCHEMA_VERSION);
        assert_eq!(loss.formalization.specification_space.symbol, "S");
        assert!(loss.loss_components.is_empty());
        assert_eq!(loss.total_magnitude(), 1.0);
    }
}
