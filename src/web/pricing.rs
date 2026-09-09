use std::{collections::BTreeMap, time::Duration};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::usage::Usage;

const MODELS_DEV_API: &str = "https://models.dev/api.json";
const MAX_PRESET_RESULTS: usize = 24;
const MAX_CATALOG_BYTES: u64 = 16 * 1024 * 1024;

/// User-supplied prices for one token. An absent value means that cost is not
/// configured; zero remains a real, explicitly configured price.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TokenPricing {
    pub uncached_input: String,
    pub cached_input: String,
    pub output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Currency {
    Usd,
    Cny,
}

impl Currency {
    const fn symbol(self) -> &'static str {
        match self {
            Self::Usd => "$",
            Self::Cny => "￥",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ParsedPricing {
    currency: Currency,
    uncached_input: f64,
    cached_input: f64,
    output: f64,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct CostEstimate {
    currency: &'static str,
    amount: f64,
    incomplete_usage: bool,
}

impl TokenPricing {
    pub fn validate(&self) -> Result<(), String> {
        self.parse().map(|_| ())
    }

    pub fn estimate(&self, usage: &Usage) -> Result<CostEstimate, String> {
        let prices = self.parse()?;
        let cached = usage
            .known_cached_input_tokens
            .min(usage.known_input_tokens);
        let uncached = usage.known_input_tokens - cached;
        let amount = uncached as f64 * prices.uncached_input
            + cached as f64 * prices.cached_input
            + usage.known_output_tokens as f64 * prices.output;
        Ok(CostEstimate {
            currency: prices.currency.symbol(),
            amount,
            incomplete_usage: usage.missing_input_usage > 0
                || usage.missing_cached_input_usage > 0
                || usage.missing_output_usage > 0
                || usage.known_cached_input_tokens > usage.known_input_tokens,
        })
    }

    fn parse(&self) -> Result<ParsedPricing, String> {
        let (uncached_currency, uncached_input) =
            parse_price("Uncached input price/token", &self.uncached_input)?;
        let (cached_currency, cached_input) =
            parse_price("Cached input price/token", &self.cached_input)?;
        let (output_currency, output) = parse_price("Output price/token", &self.output)?;
        if uncached_currency != cached_currency || uncached_currency != output_currency {
            return Err("All three token prices must use the same currency".into());
        }
        Ok(ParsedPricing {
            currency: uncached_currency,
            uncached_input,
            cached_input,
            output,
        })
    }
}

fn parse_price(label: &str, value: &str) -> Result<(Currency, f64), String> {
    let value = value.trim();
    let (currency, number) = if let Some(number) = value.strip_prefix('$') {
        (Currency::Usd, number)
    } else if let Some(number) = value.strip_prefix('￥').or_else(|| value.strip_prefix('¥')) {
        (Currency::Cny, number)
    } else {
        return Err(format!("{label} must start with $ or ￥"));
    };
    let number = number
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("{label} must contain a valid number"))?;
    if !number.is_finite() || number < 0.0 {
        return Err(format!("{label} must be a finite non-negative number"));
    }
    Ok((currency, number))
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct PricingPreset {
    provider_id: String,
    provider_name: String,
    model_id: String,
    model_name: String,
    uncached_input: String,
    cached_input: String,
    output: String,
    cached_price_assumed: bool,
    tiered_pricing: bool,
}

#[derive(Debug, Deserialize)]
struct ProviderRecord {
    id: String,
    name: String,
    #[serde(default)]
    models: BTreeMap<String, ModelRecord>,
}

#[derive(Debug, Deserialize)]
struct ModelRecord {
    id: String,
    name: String,
    #[serde(default)]
    cost: Option<ModelCost>,
}

#[derive(Debug, Deserialize)]
struct ModelCost {
    input: f64,
    output: f64,
    #[serde(default)]
    cache_read: Option<f64>,
    #[serde(default)]
    tiers: Vec<serde_json::Value>,
}

pub(super) struct PricingCatalog {
    client: reqwest::Client,
    presets: Mutex<Option<Vec<PricingPreset>>>,
}

impl PricingCatalog {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .user_agent(concat!("BeyondSlides/", env!("CARGO_PKG_VERSION")))
                .build()?,
            presets: Mutex::default(),
        })
    }

    pub async fn search(&self, query: &str) -> Result<Vec<PricingPreset>, String> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut stored = self.presets.lock().await;
        if stored.is_none() {
            let response = self
                .client
                .get(MODELS_DEV_API)
                .send()
                .await
                .map_err(|error| format!("could not load model-price presets: {error}"))?
                .error_for_status()
                .map_err(|error| format!("could not load model-price presets: {error}"))?;
            if response
                .content_length()
                .is_some_and(|length| length > MAX_CATALOG_BYTES)
            {
                return Err("Models.dev returned an unexpectedly large catalog".into());
            }
            let body = response
                .bytes()
                .await
                .map_err(|error| format!("could not read model-price presets: {error}"))?;
            if body.len() as u64 > MAX_CATALOG_BYTES {
                return Err("Models.dev returned an unexpectedly large catalog".into());
            }
            *stored = Some(parse_catalog(&body)?);
        }
        Ok(search_presets(stored.as_deref().unwrap_or_default(), query))
    }
}

fn parse_catalog(bytes: &[u8]) -> Result<Vec<PricingPreset>, String> {
    let providers: BTreeMap<String, ProviderRecord> = serde_json::from_slice(bytes)
        .map_err(|error| format!("Models.dev returned an invalid catalog: {error}"))?;
    let mut presets = Vec::new();
    for provider in providers.into_values() {
        for model in provider.models.into_values() {
            let Some(cost) = model.cost else {
                continue;
            };
            if !cost.input.is_finite()
                || !cost.output.is_finite()
                || cost.input < 0.0
                || cost.output < 0.0
            {
                continue;
            }
            let published_cached = cost
                .cache_read
                .filter(|price| price.is_finite() && *price >= 0.0);
            let cached_price_assumed = published_cached.is_none();
            let cached = published_cached.unwrap_or(cost.input);
            presets.push(PricingPreset {
                provider_id: provider.id.clone(),
                provider_name: provider.name.clone(),
                model_id: model.id,
                model_name: model.name,
                uncached_input: per_token_usd(cost.input),
                cached_input: per_token_usd(cached),
                output: per_token_usd(cost.output),
                cached_price_assumed,
                tiered_pricing: !cost.tiers.is_empty(),
            });
        }
    }
    Ok(presets)
}

fn per_token_usd(per_million_tokens: f64) -> String {
    let mut value = format!("{:.15}", per_million_tokens / 1_000_000.0);
    while value.ends_with('0') {
        value.pop();
    }
    if value.ends_with('.') {
        value.push('0');
    }
    format!("${value}")
}

fn search_presets(presets: &[PricingPreset], query: &str) -> Vec<PricingPreset> {
    let terms: Vec<_> = query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let query = query.to_lowercase();
    let mut matches: Vec<_> = presets
        .iter()
        .filter_map(|preset| {
            let haystack = format!(
                "{} {} {} {}",
                preset.provider_id, preset.provider_name, preset.model_id, preset.model_name
            )
            .to_lowercase();
            if !terms.iter().all(|term| haystack.contains(term)) {
                return None;
            }
            let model_id = preset.model_id.to_lowercase();
            let model_name = preset.model_name.to_lowercase();
            let rank = if model_id == query || model_name == query {
                0
            } else if model_id.starts_with(&query) || model_name.starts_with(&query) {
                1
            } else {
                2
            };
            Some((rank, preset))
        })
        .collect();
    matches.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.provider_name.cmp(&right.1.provider_name))
            .then_with(|| left.1.model_name.cmp(&right.1.model_name))
    });
    matches
        .into_iter()
        .take(MAX_PRESET_RESULTS)
        .map(|(_, preset)| preset.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pricing(currency: &str) -> TokenPricing {
        TokenPricing {
            uncached_input: format!("{currency}0.000001"),
            cached_input: format!("{currency}0.0000002"),
            output: format!("{currency}0.000003"),
        }
    }

    #[test]
    fn prices_require_one_supported_currency_for_all_three_fields() {
        assert!(pricing("$").validate().is_ok());
        assert!(pricing("￥").validate().is_ok());
        assert!(pricing("¥").validate().is_ok());
        let mut mixed = pricing("$");
        mixed.output = "￥0.1".into();
        assert!(mixed.validate().unwrap_err().contains("same currency"));
        let mut missing_symbol = pricing("$");
        missing_symbol.output = "0.1".into();
        assert!(
            missing_symbol
                .validate()
                .unwrap_err()
                .contains("start with")
        );
        let mut negative = pricing("$");
        negative.output = "$-1".into();
        assert!(negative.validate().unwrap_err().contains("non-negative"));
    }

    #[test]
    fn configured_zero_is_distinct_from_absent_pricing() {
        let free = TokenPricing {
            uncached_input: "$0".into(),
            cached_input: "$0".into(),
            output: "$0".into(),
        };
        let estimate = free.estimate(&Usage::default()).unwrap();
        assert_eq!(estimate.amount, 0.0);
        assert_eq!(estimate.currency, "$");
    }

    #[test]
    fn estimate_separates_cached_and_uncached_input() {
        let usage = Usage {
            responses: 1,
            known_input_tokens: 100,
            known_cached_input_tokens: 40,
            known_output_tokens: 10,
            ..Usage::default()
        };
        let estimate = pricing("$").estimate(&usage).unwrap();
        assert!((estimate.amount - 0.000_098).abs() < f64::EPSILON);
        assert!(!estimate.incomplete_usage);
    }

    #[test]
    fn catalog_converts_per_million_prices_and_marks_cache_fallbacks() {
        let presets = parse_catalog(
            br#"{
              "provider": {"id":"provider","name":"Provider","models": {
                "a": {"id":"model-a","name":"Model A","cost":{"input":1,"output":3,"cache_read":0.2}},
                "b": {"id":"model-b","name":"Model B","cost":{"input":2,"output":4,"tiers":[{"context":200000}]}}
              }}
            }"#,
        )
        .unwrap();
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0].uncached_input, "$0.000001");
        assert_eq!(presets[0].cached_input, "$0.0000002");
        assert!(!presets[0].cached_price_assumed);
        assert_eq!(presets[1].cached_input, "$0.000002");
        assert!(presets[1].cached_price_assumed);
        assert!(presets[1].tiered_pricing);
    }

    #[test]
    fn search_requires_every_term_and_prefers_exact_models() {
        let presets = parse_catalog(
            br#"{
              "one": {"id":"one","name":"First Provider","models": {
                "a": {"id":"glm-5","name":"GLM 5","cost":{"input":1,"output":2}}
              }},
              "two": {"id":"two","name":"Second Provider","models": {
                "a": {"id":"vendor/glm-5-fast","name":"GLM 5 Fast","cost":{"input":2,"output":3}}
              }}
            }"#,
        )
        .unwrap();
        let found = search_presets(&presets, "glm-5");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].model_id, "glm-5");
        assert_eq!(search_presets(&presets, "second fast").len(), 1);
    }
}
