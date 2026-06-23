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
    pub text: String,
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
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct EquationSummary {
    pub id: EquationId,
    pub name: String,
    pub description: String,
    pub latex: String,
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
            created_at: now.clone(),
            updated_at: now,
        }
    }
}
