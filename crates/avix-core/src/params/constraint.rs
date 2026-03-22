use serde::{Deserialize, Serialize};

/// A numeric min/max (inclusive) constraint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RangeConstraint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

impl RangeConstraint {
    /// Returns the tightest (intersection) range of `self` and `other`.
    /// For min: highest value wins. For max: lowest value wins.
    pub fn intersect(&self, other: &Self) -> Self {
        let min = match (self.min, other.min) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let max = match (self.max, other.max) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        Self { min, max }
    }

    /// Clamp `value` into `[min, max]`. Returns the clamped value.
    pub fn clamp(&self, value: f64) -> f64 {
        let v = if let Some(lo) = self.min {
            value.max(lo)
        } else {
            value
        };
        if let Some(hi) = self.max {
            v.min(hi)
        } else {
            v
        }
    }

    /// Returns `true` if `value` satisfies this constraint.
    pub fn allows(&self, value: f64) -> bool {
        if let Some(lo) = self.min {
            if value < lo {
                return false;
            }
        }
        if let Some(hi) = self.max {
            if value > hi {
                return false;
            }
        }
        true
    }
}

/// A constraint that restricts a value to a fixed set of allowed strings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnumConstraint {
    pub values: Vec<String>,
}

impl EnumConstraint {
    /// Returns the intersection of allowed values.
    pub fn intersect(&self, other: &Self) -> Self {
        let values = self
            .values
            .iter()
            .filter(|v| other.values.contains(v))
            .cloned()
            .collect();
        Self { values }
    }

    /// Returns `true` if `value` is in the allowed set.
    pub fn allows(&self, value: &str) -> bool {
        self.values.iter().any(|v| v == value)
    }
}

/// A constraint on a list — items must be in the allowed set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SetConstraint {
    pub allowed: Vec<String>,
}

impl SetConstraint {
    /// Returns the intersection of allowed items.
    pub fn intersect(&self, other: &Self) -> Self {
        let allowed = self
            .allowed
            .iter()
            .filter(|v| other.allowed.contains(v))
            .cloned()
            .collect();
        Self { allowed }
    }

    /// Returns `true` if `item` is in the allowed set.
    pub fn allows(&self, item: &str) -> bool {
        self.allowed.iter().any(|v| v == item)
    }

    /// Returns only the items from `list` that are allowed.
    pub fn filter(&self, list: &[String]) -> Vec<String> {
        list.iter().filter(|v| self.allows(v)).cloned().collect()
    }
}

/// A constraint that may lock a boolean field to a specific value.
/// `value: None` means the field is unconstrained at this layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoolConstraint {
    pub value: Option<bool>,
}

impl BoolConstraint {
    /// Returns `true` if `v` satisfies this constraint.
    pub fn allows(&self, v: bool) -> bool {
        match self.value {
            Some(locked) => v == locked,
            None => true,
        }
    }

    /// Apply tightest constraint: if either side locks the value, that wins.
    pub fn intersect(&self, other: &Self) -> Self {
        // If either side is locked, that locked value wins.
        // If both sides lock to different values, self takes precedence
        // (caller should treat this as a configuration error).
        let value = self.value.or(other.value);
        Self { value }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_intersect_tightest() {
        let a = RangeConstraint {
            min: Some(1.0),
            max: Some(10.0),
        };
        let b = RangeConstraint {
            min: Some(3.0),
            max: Some(7.0),
        };
        let c = a.intersect(&b);
        assert_eq!(c.min, Some(3.0));
        assert_eq!(c.max, Some(7.0));
    }

    #[test]
    fn range_intersect_none_fields() {
        let a = RangeConstraint {
            min: None,
            max: Some(10.0),
        };
        let b = RangeConstraint {
            min: Some(2.0),
            max: None,
        };
        let c = a.intersect(&b);
        assert_eq!(c.min, Some(2.0));
        assert_eq!(c.max, Some(10.0));
    }

    #[test]
    fn range_clamp_value() {
        let r = RangeConstraint {
            min: Some(1.0),
            max: Some(5.0),
        };
        assert_eq!(r.clamp(0.0), 1.0);
        assert_eq!(r.clamp(3.0), 3.0);
        assert_eq!(r.clamp(9.0), 5.0);
    }

    #[test]
    fn enum_intersect_intersection() {
        let a = EnumConstraint {
            values: vec!["sonnet".into(), "haiku".into()],
        };
        let b = EnumConstraint {
            values: vec!["haiku".into(), "opus".into()],
        };
        let c = a.intersect(&b);
        assert_eq!(c.values, vec!["haiku"]);
    }

    #[test]
    fn set_filter_removes_disallowed() {
        let s = SetConstraint {
            allowed: vec!["web".into(), "read".into()],
        };
        let list = vec!["web".into(), "email".into(), "read".into()];
        assert_eq!(s.filter(&list), vec!["web", "read"]);
    }

    #[test]
    fn bool_constraint_locked_true_disallows_false() {
        let c = BoolConstraint { value: Some(true) };
        assert!(c.allows(true));
        assert!(!c.allows(false));
    }

    #[test]
    fn bool_constraint_none_allows_any() {
        let c = BoolConstraint { value: None };
        assert!(c.allows(true));
        assert!(c.allows(false));
    }
}
