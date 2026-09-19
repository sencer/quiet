use std::collections::HashMap;
use zvariant::OwnedValue;

/// Extracts the urgency level from D-Bus hints.
/// Urgency levels according to desktop notifications spec:
/// 0: Low, 1: Normal, 2: Critical.
pub fn extract_urgency(hints: &HashMap<String, OwnedValue>) -> u8 {
    if let Some(val) = hints.get("urgency") {
        parse_urgency_value(val)
    } else {
        1 // Default: Normal
    }
}

pub fn parse_urgency_value(val: &OwnedValue) -> u8 {
    use zvariant::Value;
    match &**val {
        Value::U8(v) => (*v).min(2),
        Value::I16(v) => (*v).clamp(0, 2) as u8,
        Value::U16(v) => (*v).clamp(0, 2) as u8,
        Value::I32(v) => (*v).clamp(0, 2) as u8,
        Value::U32(v) => (*v).clamp(0, 2) as u8,
        Value::I64(v) => (*v).clamp(0, 2) as u8,
        Value::U64(v) => (*v).clamp(0, 2) as u8,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_urgency() {
        let mut hints = HashMap::new();
        assert_eq!(extract_urgency(&hints), 1);

        hints.insert("urgency".to_string(), OwnedValue::from(2u8));
        assert_eq!(extract_urgency(&hints), 2);

        hints.insert("urgency".to_string(), OwnedValue::from(0i32));
        assert_eq!(extract_urgency(&hints), 0);

        hints.insert("urgency".to_string(), OwnedValue::from(255u8));
        assert_eq!(extract_urgency(&hints), 2);
    }
}
