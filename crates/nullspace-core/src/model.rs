use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EquationId(pub Uuid);

impl EquationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s).ok().map(Self)
    }
}

impl Default for EquationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for EquationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Variable {
    pub symbol: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reference {
    #[serde(default)]
    pub authors: String,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(alias = "text", default)]
    pub title: String,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Equation {
    pub id: EquationId,
    pub name: String,
    pub description: String,
    pub latex: String,
    pub references: Vec<Reference>,
    pub tags: Vec<String>,
    pub variables: Vec<Variable>,
    pub related: Vec<EquationId>,
    pub px_height: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct EquationSummary {
    pub id: EquationId,
    pub name: String,
    pub description: String,
    pub latex: String,
    pub unicode_approx: String,
    pub px_height: u32,
}

impl Equation {
    pub fn new(name: String, latex: String) -> Self {
        let now = crate::store::now_rfc3339();
        Self {
            id: EquationId::new(),
            name,
            description: String::new(),
            latex,
            references: Vec::new(),
            tags: Vec::new(),
            variables: Vec::new(),
            related: Vec::new(),
            px_height: 48,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

#[cfg(test)]
mod reference_compat_tests {
    use super::Reference;

    #[test]
    fn reference_reads_legacy_text_field() {
        let json =
            r#"{"text":"Kohn & Sham 1965","url":"https://doi.org/10.1103/PhysRev.140.A1133"}"#;

        let r: Reference = serde_json::from_str(json).unwrap();

        assert_eq!(r.title, "Kohn & Sham 1965");
        assert_eq!(
            r.url.as_deref(),
            Some("https://doi.org/10.1103/PhysRev.140.A1133")
        );
        assert!(r.authors.is_empty());
        assert!(r.year.is_none());
        assert!(r.doi.is_none());
    }
}
