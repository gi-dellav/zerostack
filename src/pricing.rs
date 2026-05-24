/// Returns (input_price_per_1M_tokens, output_price_per_1M_tokens).
/// Uses model name heuristics to match known models; unknown models return None
/// so callers can fall back to an estimate.
pub fn model_pricing(model: &str) -> Option<(f64, f64)> {
    // Normalize for matching
    let m = model.to_lowercase();

    // DeepSeek
    if m.contains("deepseek-chat") || m.contains("deepseek-v4") {
        return Some((0.27, 1.10));
    }
    if m.contains("deepseek-reasoner") || m.contains("deepseek-r1") {
        return Some((0.55, 2.19));
    }

    // OpenAI
    if m.contains("gpt-4o-mini") {
        return Some((0.15, 0.60));
    }
    if m.contains("gpt-4o") {
        return Some((2.50, 10.00));
    }
    if m.contains("gpt-4.1-nano") {
        return Some((0.10, 0.40));
    }
    if m.contains("gpt-4.1-mini") {
        return Some((0.40, 1.60));
    }
    if m.contains("gpt-4.1") {
        return Some((2.00, 8.00));
    }
    if m.contains("o4-mini") {
        return Some((1.10, 4.40));
    }
    if m.contains("o3") || m.contains("o1") {
        return Some((10.00, 40.00));
    }

    // Anthropic
    if m.contains("claude-sonnet-4") || m.contains("claude-4-sonnet") {
        return Some((3.00, 15.00));
    }
    if m.contains("claude-haiku-3.5") || m.contains("claude-3.5-haiku") {
        return Some((0.80, 4.00));
    }
    if m.contains("claude-opus-4") || m.contains("claude-4-opus") {
        return Some((15.00, 75.00));
    }
    if m.contains("claude-sonnet-3.5") || m.contains("claude-3.5-sonnet") {
        return Some((3.00, 15.00));
    }
    if m.contains("claude") {
        return Some((3.00, 15.00));
    }

    // Gemini
    if m.contains("gemini-2.5-pro") {
        return Some((1.25, 10.00));
    }
    if m.contains("gemini-2.5-flash") {
        return Some((0.15, 0.60));
    }
    if m.contains("gemini-2.0-flash") || m.contains("gemini-2.0-flash-lite") {
        return Some((0.10, 0.40));
    }
    if m.contains("gemini") {
        return Some((0.10, 0.40));
    }

    None
}

/// Estimate cost for a given model and token counts.
/// Returns 0.0 if pricing is unknown.
pub fn estimate_cost(model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    match model_pricing(model) {
        Some((input_price, output_price)) => {
            let input_cost = input_tokens as f64 * input_price / 1_000_000.0;
            let output_cost = output_tokens as f64 * output_price / 1_000_000.0;
            input_cost + output_cost
        }
        None => 0.0,
    }
}
