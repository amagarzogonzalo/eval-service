use crate::error::UsageError;
use serde::{Deserialize, Serialize};

/// Versioned pricing table. Costs are estimated at aggregation time
/// (never at import), recording the pricing version used — so a price
/// change never rewrites history, and two aggregations of the same
/// events under different versions are comparable and explainable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceTable {
    pub pricing_version: u32,
    #[serde(default)]
    pub currency: String,
    #[serde(default)]
    pub rates: Vec<PriceRate>,
}

/// USD (or `currency`) per 1M tokens. `"*"` wildcards provider/model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceRate {
    pub provider: String,
    pub model: String,
    pub input_per_1m: f64,
    pub output_per_1m: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provenance {
    Reported,
    Estimated,
}

/// A cost figure with its source attached. Display code must show the
/// provenance next to the number.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CostEstimate {
    pub usd: f64,
    pub provenance: Provenance,
    pub pricing_version: Option<u32>,
}

impl PriceTable {
    /// Load and validate a pricing TOML file.
    pub fn load(path: &std::path::Path) -> Result<Self, UsageError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| UsageError::io(path.display().to_string(), e))?;
        let table: Self =
            toml::from_str(&text).map_err(|e| UsageError::Config(format!("pricing: {e}")))?;
        table.validate()?;
        Ok(table)
    }

    fn validate(&self) -> Result<(), UsageError> {
        if self.pricing_version < 1 {
            return Err(UsageError::Config(
                "pricing_version must be >= 1".to_string(),
            ));
        }
        for rate in &self.rates {
            if rate.input_per_1m < 0.0 || rate.output_per_1m < 0.0 {
                return Err(UsageError::Config(format!(
                    "negative rate for {}/{}",
                    rate.provider, rate.model
                )));
            }
        }
        Ok(())
    }

    /// Resolve (input, output) rates: exact provider+model first,
    /// then the provider default, then the global wildcard. `None`
    /// means unknown — the caller must leave cost absent, never zero-fill.
    pub fn resolve(&self, provider: &str, model: &str) -> Option<(f64, f64)> {
        let mut provider_default: Option<(f64, f64)> = None;
        let mut global: Option<(f64, f64)> = None;
        for rate in &self.rates {
            if rate.provider == provider && rate.model == model {
                return Some((rate.input_per_1m, rate.output_per_1m));
            }
            if rate.provider == provider && rate.model == "*" && provider_default.is_none() {
                provider_default = Some((rate.input_per_1m, rate.output_per_1m));
            }
            if rate.provider == "*" && rate.model == "*" && global.is_none() {
                global = Some((rate.input_per_1m, rate.output_per_1m));
            }
        }
        provider_default.or(global)
    }

    /// Estimate cost for token counts under this table version.
    pub fn estimate(
        &self,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Option<CostEstimate> {
        let (price_in, price_out) = self.resolve(provider, model)?;
        Some(CostEstimate {
            usd: (input_tokens as f64 / 1_000_000.0) * price_in
                + (output_tokens as f64 / 1_000_000.0) * price_out,
            provenance: Provenance::Estimated,
            pricing_version: Some(self.pricing_version),
        })
    }

    /// An empty table: everything stays cost-unknown.
    pub fn empty() -> Self {
        Self {
            pricing_version: 1,
            currency: "USD".to_string(),
            rates: Vec::new(),
        }
    }
}
