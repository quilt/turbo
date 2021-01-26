use serde::{Deserialize, Serialize};

use typed_builder::TypedBuilder;

#[derive(Debug, TypedBuilder, Serialize, Deserialize)]
pub struct Config {
    max_txs: usize,

    #[builder(setter(into))]
    control: String,
}

impl Config {
    pub fn max_txs(&self) -> usize {
        self.max_txs
    }

    pub fn control(&self) -> &str {
        &self.control
    }
}
