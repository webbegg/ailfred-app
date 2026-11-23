//! A line against the official AG-UI 1.0 schema, for tests (this crate's, and the app's
//! server's with the `testing` feature): a pragmatic checker, not a validator. It
//! picks the definition an object is (by its `type` or `role` const, through `$ref`, `allOf` and
//! `oneOf`), keeps the keywords beside a `$ref` too, and fails on a key that definition doesn't
//! declare, a required one missing, a `const` or `enum` value that differs, a JSON `type` that
//! doesn't match, a value `not` refuses, a number past `minimum`/`maximum`, or an array shorter
//! than `minItems`; open objects (metadata, activity content, a carried schema) aren't looked
//! into. `pattern` isn't checked: that needs a regex crate, and the crate takes no more
//! dependencies (the one pattern, a JSON Pointer's, is on events Ailfred doesn't send).

use std::cmp::Ordering;

use serde_json::{Map, Number, Value, json};

const SPEC: &str = include_str!("../spec/ag-ui-1.0.schema.json");

pub struct Spec {
    pub(crate) defs: Map<String, Value>,
}

/// The properties a definition declares, the ones it requires, and whether it's open (takes
/// keys it doesn't declare).
struct Shape {
    props: Map<String, Value>,
    required: Vec<String>,
    open: bool,
}

impl Spec {
    pub fn load() -> Self {
        let spec: Value = serde_json::from_str(SPEC).expect("the spec is JSON");
        Spec { defs: spec["$defs"].as_object().expect("the spec has $defs").clone() }
    }

    /// What's wrong with `value` as definition `def` (`Event`, `RunAgentInput`, …).
    pub fn errors(&self, value: &Value, def: &str) -> Vec<String> {
        let mut errors = Vec::new();
        self.check(value, &json!({ "$ref": format!("#/$defs/{def}") }), def, &mut errors);
        errors
    }

    /// `schema`, with the definition it refers to in place of its `$ref`: the keywords beside the
    /// `$ref` hold as well (an `allOf` of both); annotations are dropped.
    fn resolve(&self, schema: &Value) -> Value {
        let Some(r) = schema.get("$ref").and_then(Value::as_str) else { return schema.clone() };
        let def = self.resolve(self.defs.get(r.trim_start_matches("#/$defs/")).unwrap_or_else(|| panic!("no definition {r}")));
        let mut siblings = schema.as_object().cloned().unwrap_or_default();
        for key in ["$ref", "$anchor", "$comment", "description", "title"] {
            siblings.remove(key);
        }
        if siblings.is_empty() { def } else { json!({ "allOf": [def, Value::Object(siblings)] }) }
    }

    /// What a definition declares, its `allOf` parts' included (its own win).
    fn shape(&self, schema: &Value) -> Shape {
        let mut shape = Shape { props: Map::new(), required: Vec::new(), open: schema.get("additionalProperties") == Some(&Value::Bool(true)) };
        for part in schema.get("allOf").and_then(Value::as_array).into_iter().flatten() {
            let part = self.shape(&self.resolve(part));
            shape.props.extend(part.props);
            shape.required.extend(part.required);
            shape.open |= part.open;
        }
        if let Some(own) = schema.get("properties").and_then(Value::as_object) {
            shape.props.extend(own.clone());
        }
        shape.required.extend(schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(|r| r.as_str().map(str::to_owned)));
        shape
    }

    /// Whether `value` is the `oneOf` branch `branch`: of its JSON type, and an object with its
    /// discriminating consts.
    fn fits(&self, value: &Value, branch: &Value) -> bool {
        match (value, branch.get("type").and_then(Value::as_str)) {
            (Value::Object(map), _) => {
                let props = self.shape(branch).props;
                let consts: Vec<(&String, &Value)> = props.iter().filter_map(|(k, p)| Some((k, p.get("const")?))).collect();
                !consts.is_empty() && consts.iter().all(|(k, c)| map.get(*k) == Some(c))
            }
            (v, Some(t)) => is_type(v, t),
            _ => false,
        }
    }

    /// Whether `value` keeps to `schema`.
    fn valid(&self, value: &Value, schema: &Value) -> bool {
        let mut errors = Vec::new();
        self.check(value, schema, "", &mut errors);
        errors.is_empty()
    }

    /// The keywords about `value` itself (not its properties or items), in `schema` and its
    /// `allOf` parts; whether they all hold.
    fn holds(&self, value: &Value, schema: &Value, path: &str, errors: &mut Vec<String>) -> bool {
        let before = errors.len();
        if let Some(c) = schema.get("const")
            && value != c
        {
            errors.push(format!("{path}: {value} is not {c}"));
        }
        if let Some(values) = schema.get("enum").and_then(Value::as_array)
            && !values.contains(value)
        {
            errors.push(format!("{path}: {value} is none of {values:?}"));
        }
        let types: Vec<&str> = match schema.get("type") {
            Some(Value::String(t)) => vec![t.as_str()],
            Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !types.is_empty() && !types.iter().any(|t| is_type(value, t)) {
            errors.push(format!("{path}: {value} is not of type {}", types.join(" or ")));
        }
        if let Some(refused) = schema.get("not")
            && self.valid(value, refused)
        {
            errors.push(format!("{path}: {value} is what `not` refuses ({refused})"));
        }
        if let Value::Number(n) = value {
            if let Some(Value::Number(min)) = schema.get("minimum")
                && compare(n, min) == Some(Ordering::Less)
            {
                errors.push(format!("{path}: {n} is below the minimum {min}"));
            }
            if let Some(Value::Number(max)) = schema.get("maximum")
                && compare(n, max) == Some(Ordering::Greater)
            {
                errors.push(format!("{path}: {n} is above the maximum {max}"));
            }
        }
        if let (Value::Array(items), Some(min)) = (value, schema.get("minItems").and_then(Value::as_u64))
            && (items.len() as u64) < min
        {
            errors.push(format!("{path}: {} items, fewer than {min}", items.len()));
        }
        for part in schema.get("allOf").and_then(Value::as_array).into_iter().flatten() {
            self.holds(value, &self.resolve(part), path, errors);
        }
        errors.len() == before
    }

    pub(crate) fn check(&self, value: &Value, schema: &Value, path: &str, errors: &mut Vec<String>) {
        let schema = self.resolve(schema);
        if !self.holds(value, &schema, path, errors) {
            return;
        }
        if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
            match branches.iter().map(|b| self.resolve(b)).find(|b| self.fits(value, b)) {
                Some(branch) => self.check(value, &branch, path, errors),
                None => errors.push(format!("{path}: no definition fits {value}")),
            }
            return;
        }
        match value {
            Value::Object(map) => {
                let Shape { props, required, open } = self.shape(&schema);
                if open || props.is_empty() {
                    return; // open by meaning
                }
                for key in map.keys().filter(|k| !props.contains_key(*k)) {
                    errors.push(format!("{path}: `{key}` is not a declared property"));
                }
                for key in required.iter().filter(|k| !map.contains_key(*k)) {
                    errors.push(format!("{path}: required `{key}` is missing"));
                }
                for (key, v) in map {
                    if let Some(p) = props.get(key) {
                        self.check(v, p, &format!("{path}.{key}"), errors);
                    }
                }
            }
            Value::Array(items) => {
                if let Some(item) = schema.get("items") {
                    for (i, v) in items.iter().enumerate() {
                        self.check(v, item, &format!("{path}[{i}]"), errors);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Whether `value` is of JSON Schema type `t` (an integer is a number too).
fn is_type(value: &Value, t: &str) -> bool {
    match (value, t) {
        (Value::Null, "null") | (Value::Bool(_), "boolean") | (Value::String(_), "string") | (Value::Array(_), "array") | (Value::Object(_), "object") | (Value::Number(_), "number") => true,
        (Value::Number(n), "integer") => n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0),
        _ => false,
    }
}

/// Two JSON numbers in order, exactly when both are integers.
fn compare(a: &Number, b: &Number) -> Option<Ordering> {
    let int = |n: &Number| n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from));
    match (int(a), int(b)) {
        (Some(a), Some(b)) => Some(a.cmp(&b)),
        _ => a.as_f64()?.partial_cmp(&b.as_f64()?),
    }
}
