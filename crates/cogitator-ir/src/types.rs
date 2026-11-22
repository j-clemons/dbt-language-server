//! The SQL type lattice: `Null` is the bottom, `Unknown` the top, and
//! [`SqlType::join`] the least upper bound lowering uses for set ops,
//! CASE arms and coalesce-shaped expressions.

use std::sync::Arc;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Tz {
    With,
    Without,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Field {
    pub name: Arc<str>,
    pub ty: SqlType,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum SqlType {
    Unknown,
    Null,
    Bool,
    Int {
        bits: Option<u8>,
    },
    Decimal {
        p: Option<u8>,
        s: Option<u8>,
    },
    Float {
        bits: Option<u8>,
    },
    Text,
    Binary,
    Date,
    Time,
    Timestamp {
        tz: Tz,
    },
    Interval,
    Json,
    Array(Arc<SqlType>),
    Struct(Arc<[Field]>),
    Map(Arc<(SqlType, SqlType)>),
    /// A type the parser recognised but the lattice does not model;
    /// joins to `Unknown` against anything else.
    Other(Arc<str>),
}

impl SqlType {
    /// Least upper bound. `Int ⊔ Decimal = Decimal`, numeric ⊔ `Float`
    /// = `Float`, `Null` is the identity, incompatible pairs are
    /// `Unknown`. Commutative and associative on the modelled types.
    pub fn join(&self, other: &SqlType) -> SqlType {
        use SqlType as T;
        match (self, other) {
            (T::Null, x) | (x, T::Null) => x.clone(),
            (T::Unknown, _) | (_, T::Unknown) => T::Unknown,
            (T::Int { bits: a }, T::Int { bits: b }) => T::Int {
                bits: join_width(*a, *b),
            },
            (T::Float { bits: a }, T::Float { bits: b }) => T::Float {
                bits: join_width(*a, *b),
            },
            (T::Decimal { p: p1, s: s1 }, T::Decimal { p: p2, s: s2 }) => {
                let (p, s) = join_decimal((*p1, *s1), (*p2, *s2));
                T::Decimal { p, s }
            }
            (T::Int { bits }, T::Decimal { p, s }) | (T::Decimal { p, s }, T::Int { bits }) => {
                let (p, s) = join_decimal((int_digits(*bits), Some(0)), (*p, *s));
                T::Decimal { p, s }
            }
            (T::Float { bits }, T::Int { .. } | T::Decimal { .. })
            | (T::Int { .. } | T::Decimal { .. }, T::Float { bits }) => T::Float { bits: *bits },
            (T::Timestamp { tz: a }, T::Timestamp { tz: b }) => T::Timestamp {
                tz: if a == b { *a } else { Tz::Unknown },
            },
            (T::Array(a), T::Array(b)) => T::Array(Arc::new(a.join(b))),
            (T::Map(a), T::Map(b)) => T::Map(Arc::new((a.0.join(&b.0), a.1.join(&b.1)))),
            (T::Struct(a), T::Struct(b)) => join_struct(a, b),
            (a, b) if a == b => a.clone(),
            _ => T::Unknown,
        }
    }

    /// True when `self` says at least as much as `old`: `self ⊔ old == old`.
    pub fn refines(&self, old: &SqlType) -> bool {
        self.join(old) == *old
    }
}

/// `None` (unstated width) absorbs: a stated width would claim more
/// than the inputs guarantee.
fn join_width(a: Option<u8>, b: Option<u8>) -> Option<u8> {
    Some(a?.max(b?))
}

/// Decimal digits needed to hold any value of an integer width.
fn int_digits(bits: Option<u8>) -> Option<u8> {
    Some(match bits? {
        0..=8 => 3,
        9..=16 => 5,
        17..=32 => 10,
        33..=64 => 19,
        _ => 39,
    })
}

/// Keeps the larger integer part and the larger scale, saturating at
/// `u8::MAX`; an unstated precision or scale stays unstated.
fn join_decimal(
    a: (Option<u8>, Option<u8>),
    b: (Option<u8>, Option<u8>),
) -> (Option<u8>, Option<u8>) {
    let s = join_width(a.1, b.1);
    let p = match (a, b, s) {
        ((Some(p1), Some(s1)), (Some(p2), Some(s2)), Some(s)) => {
            let int = p1.saturating_sub(s1).max(p2.saturating_sub(s2));
            Some(int.saturating_add(s))
        }
        _ => None,
    };
    (p, s)
}

fn join_struct(a: &Arc<[Field]>, b: &Arc<[Field]>) -> SqlType {
    let same_shape = a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.name == y.name);
    if !same_shape {
        return SqlType::Unknown;
    }
    let fields: Vec<Field> = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| Field {
            name: Arc::clone(&x.name),
            ty: x.ty.join(&y.ty),
        })
        .collect();
    SqlType::Struct(Arc::from(fields))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Nullability {
    NotNull,
    Nullable,
    Unknown,
}

impl Nullability {
    pub fn join(self, other: Nullability) -> Nullability {
        use Nullability as N;
        match (self, other) {
            (N::Unknown, _) | (_, N::Unknown) => N::Unknown,
            (N::Nullable, _) | (_, N::Nullable) => N::Nullable,
            (N::NotNull, N::NotNull) => N::NotNull,
        }
    }

    pub fn refines(self, old: Nullability) -> bool {
        self.join(old) == old
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ColType {
    pub ty: SqlType,
    pub null: Nullability,
}

impl ColType {
    pub const UNKNOWN: ColType = ColType {
        ty: SqlType::Unknown,
        null: Nullability::Unknown,
    };

    pub fn new(ty: SqlType, null: Nullability) -> Self {
        Self { ty, null }
    }

    pub fn join(&self, other: &ColType) -> ColType {
        ColType {
            ty: self.ty.join(&other.ty),
            null: self.null.join(other.null),
        }
    }

    /// The MIR "types never lose specificity" order.
    pub fn refines(&self, old: &ColType) -> bool {
        self.ty.refines(&old.ty) && self.null.refines(old.null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SqlType as T;

    fn int(b: u8) -> SqlType {
        T::Int { bits: Some(b) }
    }
    fn dec(p: u8, s: u8) -> SqlType {
        T::Decimal {
            p: Some(p),
            s: Some(s),
        }
    }
    fn float(b: u8) -> SqlType {
        T::Float { bits: Some(b) }
    }

    #[test]
    fn numeric_widening() {
        assert_eq!(int(32).join(&int(64)), int(64));
        assert_eq!(int(32).join(&T::Int { bits: None }), T::Int { bits: None });
        assert_eq!(int(32).join(&dec(12, 2)), dec(12, 2));
        assert_eq!(int(64).join(&dec(12, 2)), dec(21, 2));
        assert_eq!(dec(10, 2).join(&dec(8, 4)), dec(12, 4));
        assert_eq!(int(32).join(&float(64)), float(64));
        assert_eq!(dec(12, 2).join(&float(32)), float(32));
        assert_eq!(float(32).join(&float(64)), float(64));
    }

    #[test]
    fn null_is_identity_and_unknown_absorbs() {
        for t in [int(32), T::Text, T::Date, T::Unknown] {
            assert_eq!(t.join(&T::Null), t);
            assert_eq!(T::Null.join(&t), t);
            assert_eq!(t.join(&T::Unknown), T::Unknown);
        }
    }

    #[test]
    fn incompatible_is_unknown() {
        assert_eq!(T::Text.join(&int(32)), T::Unknown);
        assert_eq!(T::Date.join(&T::Timestamp { tz: Tz::With }), T::Unknown);
        assert_eq!(T::Bool.join(&T::Text), T::Unknown);
        let a = T::Other(Arc::from("GEOGRAPHY"));
        let b = T::Other(Arc::from("VARIANT"));
        assert_eq!(a.join(&a), a);
        assert_eq!(a.join(&b), T::Unknown);
    }

    #[test]
    fn structural_types_join_componentwise() {
        let arr = |t: SqlType| T::Array(Arc::new(t));
        assert_eq!(arr(int(32)).join(&arr(float(64))), arr(float(64)));
        let map = |k, v| T::Map(Arc::new((k, v)));
        assert_eq!(
            map(T::Text, int(32)).join(&map(T::Text, int(64))),
            map(T::Text, int(64))
        );
        let st = |n: &str, t: SqlType| {
            T::Struct(Arc::from(vec![Field {
                name: Arc::from(n),
                ty: t,
            }]))
        };
        assert_eq!(st("a", int(8)).join(&st("a", int(16))), st("a", int(16)));
        assert_eq!(st("a", int(8)).join(&st("b", int(8))), T::Unknown);
    }

    #[test]
    fn timestamp_zones_join_to_unknown_zone() {
        let ts = |tz| T::Timestamp { tz };
        assert_eq!(ts(Tz::With).join(&ts(Tz::With)), ts(Tz::With));
        assert_eq!(ts(Tz::With).join(&ts(Tz::Without)), ts(Tz::Unknown));
    }

    #[test]
    fn refinement_order() {
        assert!(int(32).refines(&T::Unknown));
        assert!(int(32).refines(&T::Int { bits: None }));
        assert!(!T::Int { bits: None }.refines(&int(32)));
        assert!(!T::Unknown.refines(&int(32)));
        let nn = ColType::new(int(32), Nullability::NotNull);
        let n = ColType::new(int(32), Nullability::Nullable);
        assert!(nn.refines(&n));
        assert!(!n.refines(&nn));
        assert!(nn.refines(&ColType::UNKNOWN));
        assert!(!ColType::UNKNOWN.refines(&nn));
    }
}
