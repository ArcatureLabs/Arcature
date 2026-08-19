use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct HomePage {
    pub name: String,
}
