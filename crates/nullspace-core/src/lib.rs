pub mod error;
pub mod identity;
pub mod model;
pub mod reference;
pub mod render;
pub mod store;

pub use error::Error;
pub use identity::equation_identity;
pub use model::{Equation, EquationId, EquationSummary, Reference, TrashEntry, Variable};
pub use reference::{format_citation, normalize_doi, normalize_pages, reference_link};
pub use store::{DuplicatePolicy, ImportSummary, Store};
