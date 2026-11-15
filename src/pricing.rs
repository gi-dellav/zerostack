/// Anthropic prompt-cache billing multipliers, relative to the base input rate.
/// Anthropic bills cache *writes* (`cache_creation_input_tokens`) at 1.25× and
/// cache *reads* (`cached_input_tokens`) at 0.10× the base input token price on
/// most models. Newer models discount cache reads further (Opus 5.5 and Sonnet
/// 5.5 bill them at 0.05×), so when the `llm-prices` feature is on the per-model
/// ratios from that table are used instead and these constants are the fallback
/// for models it does not list (`data/models.json` only has base input/output
/// prices). WRITE assumes the default 5-minute cache TTL; the 1-hour TTL bills
/// writes at 2.0× — revisit if that is ever enabled (see `provider.rs`'s
/// `.with_prompt_caching()`).
const ANTHROPIC_CACHE_WRITE_MULT: f64 = 1.25;
const ANTHROPIC_CACHE_READ_MULT: f64 = 0.10;

/// Per-model prompt-cache multipliers `(read, 5-minute write)`, as ratios of the
/// model's own base input price in the `llm-prices` table. Ratios rather than
/// absolute prices, so the base input rate still comes from `data/models.json`
/// (or the user's config) and only the cache discount comes from the table.
/// `None` when the model is not listed or has no cache-read price. DeepSeek
/// rows are skipped: their cache-hit prices change often, so those models keep
/// the previous full-rate behaviour.
#[cfg(feature = "llm-prices")]
fn cache_multipliers(model: &str) -> Option<(f64, Option<f64>)> {
    let price = llm_prices::lookup(model)?;
    if !matches!(price.provider, "anthropic" | "openai" | "google") {
        return None;
    }
    let input = price.input.filter(|&p| p > 0.0)?;
    let read = price.cache_read? / input;
    let write = price.cache_write("5m").map(|w| w / input);
    Some((read, write))
}

#[cfg(not(feature = "llm-prices"))]
fn cache_multipliers(_model: &str) -> Option<(f64, Option<f64>)> {
    None
}

/// Effective (billable) input-token count, folding prompt-cache tiers into a
/// single number priced at the base input rate. The cost-side sibling of
/// [`Session::real_input_tokens`](crate::session::Session::real_input_tokens):
/// where that one answers "how big is the context" (for compaction), this
/// answers "how much input do we pay for".
///
/// Anthropic's reported `input_tokens` *excludes* both cached and cache-creation
/// tokens, but both are billed — reads at a fraction of the input rate and
/// writes at 1.25× — so pricing on raw `input_tokens` alone undercounts
/// massively whenever caching is active (the fixed request prefix — tool defs +
/// system prompt — is re-sent every turn and lands almost entirely in the cache
/// tiers). Every other provider folds the cached subset into `input_tokens` and
/// reports no separate cache-creation; for those the cached subset is billed at
/// the model's cache-read rate when the `llm-prices` table lists the model, and
/// `input_tokens` is returned unchanged otherwise.
///
/// `anthropic_native` must be the resolved protocol route — compute it with
/// [`Config::is_anthropic_native`](crate::config::Config::is_anthropic_native),
/// exactly as `real_input_tokens` callers do. `model` is the model id used for
/// the request.
pub fn billable_input_tokens(
    anthropic_native: bool,
    model: &str,
    input_tokens: u64,
    cached_input_tokens: u64,
    cache_creation_input_tokens: u64,
) -> u64 {
    let multipliers = cache_multipliers(model);
    if anthropic_native {
        let (read_mult, write_mult) = match multipliers {
            Some((read, write)) => (read, write.unwrap_or(ANTHROPIC_CACHE_WRITE_MULT)),
            None => (ANTHROPIC_CACHE_READ_MULT, ANTHROPIC_CACHE_WRITE_MULT),
        };
        (input_tokens as f64
            + cache_creation_input_tokens as f64 * write_mult
            + cached_input_tokens as f64 * read_mult)
            .round() as u64
    } else if let Some((read_mult, _)) = multipliers {
        let cached = cached_input_tokens.min(input_tokens);
        ((input_tokens - cached) as f64 + cached as f64 * read_mult).round() as u64
    } else {
        input_tokens
    }
}

/// Estimate cost for given token counts and per-million-token prices.
/// Returns 0.0 if either price is 0.0.
///
/// `input_tokens` is the *billable* input count — for Anthropic routes, pass the
/// output of [`billable_input_tokens`] so cache reads/writes are priced, not the
/// raw provider-reported `input_tokens` (which excludes both cache tiers).
pub fn estimate_cost(
    input_tokens: u64,
    output_tokens: u64,
    input_token_cost: f64,
    output_token_cost: f64,
) -> f64 {
    let input_cost = input_tokens as f64 * input_token_cost / 1_000_000.0;
    let output_cost = output_tokens as f64 * output_token_cost / 1_000_000.0;
    input_cost + output_cost
}
