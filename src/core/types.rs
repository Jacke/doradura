use std::fmt;
use std::str::FromStr;

/// User subscription plan
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Plan {
    #[default]
    Free,
    Premium,
    Vip,
}

impl Plan {
    pub fn as_str(&self) -> &'static str {
        match self {
            Plan::Free => "free",
            Plan::Premium => "premium",
            Plan::Vip => "vip",
        }
    }

    pub fn is_paid(&self) -> bool {
        matches!(self, Plan::Premium | Plan::Vip)
    }

    pub fn emoji(&self) -> &'static str {
        match self {
            Plan::Free => "🆓",
            Plan::Premium => "⭐",
            Plan::Vip => "👑",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Plan::Free => "Free",
            Plan::Premium => "Premium",
            Plan::Vip => "VIP",
        }
    }
}

impl fmt::Display for Plan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Plan {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "free" => Ok(Plan::Free),
            "premium" => Ok(Plan::Premium),
            "vip" => Ok(Plan::Vip),
            _ => Err(format!("Unknown plan: {}", s)),
        }
    }
}

// rusqlite FromSql: read plan from DB text column
impl rusqlite::types::FromSql for Plan {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let s = value.as_str()?;
        Plan::from_str(s).map_err(|e| rusqlite::types::FromSqlError::Other(Box::new(std::io::Error::other(e))))
    }
}

// rusqlite ToSql: write plan as text to DB
impl rusqlite::types::ToSql for Plan {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Text(
            self.as_str().as_bytes(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_from_str() {
        assert_eq!(Plan::from_str("free").unwrap(), Plan::Free);
        assert_eq!(Plan::from_str("premium").unwrap(), Plan::Premium);
        assert_eq!(Plan::from_str("vip").unwrap(), Plan::Vip);
        assert!(Plan::from_str("unknown").is_err());
    }

    #[test]
    fn test_plan_display() {
        assert_eq!(Plan::Free.to_string(), "free");
        assert_eq!(Plan::Premium.to_string(), "premium");
        assert_eq!(Plan::Vip.to_string(), "vip");
    }

    #[test]
    fn test_plan_as_str() {
        assert_eq!(Plan::Free.as_str(), "free");
        assert_eq!(Plan::Premium.as_str(), "premium");
        assert_eq!(Plan::Vip.as_str(), "vip");
    }

    #[test]
    fn test_plan_is_paid() {
        assert!(!Plan::Free.is_paid());
        assert!(Plan::Premium.is_paid());
        assert!(Plan::Vip.is_paid());
    }

    #[test]
    fn test_plan_emoji() {
        assert_eq!(Plan::Free.emoji(), "🆓");
        assert_eq!(Plan::Premium.emoji(), "⭐");
        assert_eq!(Plan::Vip.emoji(), "👑");
    }

    #[test]
    fn test_plan_display_name() {
        assert_eq!(Plan::Free.display_name(), "Free");
        assert_eq!(Plan::Premium.display_name(), "Premium");
        assert_eq!(Plan::Vip.display_name(), "VIP");
    }

    #[test]
    fn test_plan_default() {
        assert_eq!(Plan::default(), Plan::Free);
    }

    #[test]
    fn test_plan_copy() {
        let plan = Plan::Premium;
        let plan2 = plan; // Copy, not move
        assert_eq!(plan, plan2);
    }
}
