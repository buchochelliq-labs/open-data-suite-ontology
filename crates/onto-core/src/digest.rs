use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Hex SHA-256 of the canonical JSON encoding of `value`: object keys are sorted
/// recursively, so the digest does not depend on input key order or on whether
/// `serde_json/preserve_order` is enabled somewhere in the dependency graph.
pub fn canonical_digest<T: Serialize + ?Sized>(value: &T) -> String {
    let value = canonicalize(serde_json::to_value(value).expect("serializable value"));
    hex_sha256(&serde_json::to_vec(&value).expect("serializable value"))
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, canonicalize(v))).collect::<Map<_, _>>())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_order_does_not_change_digest() {
        let a: Value = serde_json::from_str(r#"{"b":1,"a":{"y":2,"x":3}}"#).unwrap();
        assert_eq!(canonical_digest(&a), canonical_digest(&json!({"a": {"x": 3, "y": 2}, "b": 1})));
    }
}
