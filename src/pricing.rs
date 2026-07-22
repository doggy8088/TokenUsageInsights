use serde::Serialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct PricingRule {
    pub model_name: String,
    pub input_price: f64,
    pub cache_input_price: f64,
    pub output_price: f64,
}

#[derive(Serialize)]
pub struct PricingEntry {
    pub model_name: String,
    pub deployment_type: String,
    pub unit: String,
    pub input_price: f64,
    pub cache_input_price: f64,
    pub output_price: f64,
    pub batch_api_price: String,
}

pub fn load_pricing_rules() -> Vec<PricingRule> {
    let mut rules = Vec::new();
    let file_path =
        crate::paths::find_resource("pricing.csv").unwrap_or_else(|| PathBuf::from("pricing.csv"));
    if let Ok(file) = File::open(&file_path) {
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        if let Some(Ok(_header)) = lines.next() {
            for line in lines.map_while(Result::ok) {
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() >= 6 {
                    let input_price = parts[3].trim().parse::<f64>().unwrap_or(0.0);
                    let cache_input_price = parts[4].trim().parse::<f64>().unwrap_or(0.0);
                    let output_price = parts[5].trim().parse::<f64>().unwrap_or(0.0);
                    rules.push(PricingRule {
                        model_name: parts[0].trim().to_string(),
                        input_price,
                        cache_input_price,
                        output_price,
                    });
                }
            }
        }
    }
    if rules.is_empty() {
        rules = vec![
            PricingRule {
                model_name: "Gemini 3.5 Flash".to_string(),
                input_price: 1.50,
                cache_input_price: 0.375,
                output_price: 9.00,
            },
            PricingRule {
                model_name: "Gemini 1.5 Flash".to_string(),
                input_price: 0.075,
                cache_input_price: 0.01875,
                output_price: 0.30,
            },
            PricingRule {
                model_name: "Gemini 1.5 Pro".to_string(),
                input_price: 1.25,
                cache_input_price: 0.3125,
                output_price: 5.00,
            },
            PricingRule {
                model_name: "Gemini 2.0 Flash".to_string(),
                input_price: 0.10,
                cache_input_price: 0.025,
                output_price: 0.40,
            },
        ];
    }
    rules
}

/// Parsed long-context threshold marker from a pricing rule label.
/// `is_greater` is true for `>Nk` / `(>Nk)` and false for `<Nk` / `(<Nk)`.
/// `threshold_tokens` is the token count boundary (for example, 200_000).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ThresholdRule {
    is_greater: bool,
    threshold_tokens: u64,
}

/// Parse a model/rule label into a normalized base name and optional threshold.
///
/// The pricing CSV has used both `(>272k)` and `(>272k context length)` forms,
/// and newer rows can use a different boundary such as 200k. Parse the marker
/// instead of tying matching behavior to one specific number or suffix.
fn parse_threshold_rule(name: &str) -> (String, Option<ThresholdRule>) {
    let lower = name.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    let context_length: Vec<char> = "context length".chars().collect();
    let mut threshold = None;
    let mut cleaned = String::with_capacity(lower.len());
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c == '>' || c == '<' {
            let is_greater = c == '>';
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_ascii_whitespace() {
                j += 1;
            }

            let digits_start = j;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }

            if j > digits_start && j < chars.len() && chars[j] == 'k' {
                let digits: String = chars[digits_start..j].iter().collect();
                if let Ok(value) = digits.parse::<u64>() {
                    if threshold.is_none() {
                        threshold = Some(ThresholdRule {
                            is_greater,
                            threshold_tokens: value.saturating_mul(1_000),
                        });
                    }

                    // Preserve compatibility with the legacy
                    // `(<272k context length)` spelling. Parentheses and
                    // whitespace are discarded during normalization anyway.
                    j += 1;
                    while j < chars.len() && chars[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if chars
                        .get(j..j + context_length.len())
                        .is_some_and(|suffix| suffix == context_length.as_slice())
                    {
                        j += context_length.len();
                    }
                    while j < chars.len() && chars[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j < chars.len() && chars[j] == ')' {
                        j += 1;
                    }

                    i = j;
                    continue;
                }
            }
        }

        cleaned.push(c);
        i += 1;
    }

    let normalized = cleaned.chars().filter(|ch| ch.is_alphanumeric()).collect();
    (normalized, threshold)
}

fn threshold_matches(rule: ThresholdRule, prompt_tokens: u64) -> bool {
    if rule.is_greater {
        prompt_tokens > rule.threshold_tokens
    } else {
        // The pricing rows use `<Nk` as the short-context tier, which includes
        // the exact boundary shown by the providers as `<=Nk`.
        prompt_tokens <= rule.threshold_tokens
    }
}

fn rule_applies_to_context(
    rule_base: &str,
    rule_threshold: Option<ThresholdRule>,
    model_base: &str,
    prompt_tokens: u64,
    contains_match: bool,
) -> bool {
    if rule_base.is_empty() {
        return false;
    }

    let base_matches = if contains_match {
        model_base.contains(rule_base) || rule_base.contains(model_base)
    } else {
        rule_base == model_base
    };
    if !base_matches {
        return false;
    }

    rule_threshold
        .map(|threshold| threshold_matches(threshold, prompt_tokens))
        .unwrap_or(true)
}

/// Find a matching rule while ensuring an applicable threshold row wins over
/// an unthresholded default row, regardless of CSV ordering.
fn find_pricing_rule<'a>(
    rules: &'a [PricingRule],
    model_base: &str,
    prompt_tokens: u64,
    contains_match: bool,
) -> Option<&'a PricingRule> {
    let mut default_rule = None;

    for rule in rules {
        let (rule_base, rule_threshold) = parse_threshold_rule(&rule.model_name);
        if !rule_applies_to_context(
            &rule_base,
            rule_threshold,
            model_base,
            prompt_tokens,
            contains_match,
        ) {
            continue;
        }

        if rule_threshold.is_some() {
            return Some(rule);
        }
        if default_rule.is_none() {
            default_rule = Some(rule);
        }
    }

    default_rule
}

#[allow(dead_code)]
pub fn normalize_model_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

pub fn calculate_cost(
    rules: &[PricingRule],
    model_name: &str,
    input: u64,
    output: u64,
    cache_read: u64,
) -> Result<f64, String> {
    let (m_base, _) = parse_threshold_rule(model_name);
    if m_base.is_empty() {
        return Err(format!(
            "模型名稱為空，無法估算成本。來源模型：{}",
            model_name
        ));
    }

    // Long-context pricing tiers are based on prompt/input tokens. `input`
    // already contains cache-write tokens for parsers that expose them, while
    // cache reads are kept separately for their discounted rate. Generated
    // output does not change the prompt tier.
    let prompt_tokens = input.saturating_add(cache_read);

    // 1. Exact base name match (threshold-aware)
    let mut rule = find_pricing_rule(rules, &m_base, prompt_tokens, false);

    // 2. Fallback: contains base name match
    if rule.is_none() {
        rule = find_pricing_rule(rules, &m_base, prompt_tokens, true);
    }

    if let Some(r) = rule {
        let input_cost = (input as f64 / 1_000_000.0) * r.input_price;
        let cache_cost = (cache_read as f64 / 1_000_000.0) * r.cache_input_price;
        let output_cost = (output as f64 / 1_000_000.0) * r.output_price;
        Ok(input_cost + cache_cost + output_cost)
    } else {
        Err(format!("找不到可用的模型價格規則：{}", model_name))
    }
}

pub fn calculate_usage_cost(
    rules: &[PricingRule],
    model_name: Option<&str>,
    input: u64,
    output: u64,
    cache_read: u64,
) -> Result<f64, String> {
    if input == 0 && output == 0 && cache_read == 0 {
        return Ok(0.0);
    }

    let model_name = model_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "缺少模型名稱，無法估算成本".to_string())?;
    calculate_cost(rules, model_name, input, output, cache_read)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_token_usage_without_model_costs_zero() {
        let cost = calculate_usage_cost(&[], None, 0, 0, 0).unwrap();
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn token_usage_without_model_reports_missing_metadata() {
        let error = calculate_usage_cost(&[], None, 10, 2, 3).unwrap_err();
        assert_eq!(error, "缺少模型名稱，無法估算成本");
    }

    #[test]
    fn copilot_cli_cost_uses_non_cached_input() {
        let rules = [PricingRule {
            model_name: "MAI-Code-1-Flash".to_string(),
            input_price: 0.75,
            cache_input_price: 0.075,
            output_price: 4.50,
        }];

        let cost = calculate_usage_cost(
            &rules,
            Some("mai-code-1-flash-picker · medium"),
            42_530,
            1_370,
            401_024,
        )
        .unwrap();

        assert!((cost - 0.068_139_3).abs() < f64::EPSILON);
    }

    fn gemini_pro_rules() -> Vec<PricingRule> {
        vec![
            PricingRule {
                model_name: "Gemini 3.1 Pro (Low) (<200k)".to_string(),
                input_price: 2.00,
                cache_input_price: 0.20,
                output_price: 12.00,
            },
            PricingRule {
                model_name: "Gemini 3.1 Pro (Low) (>200k)".to_string(),
                input_price: 4.00,
                cache_input_price: 0.40,
                output_price: 18.00,
            },
            PricingRule {
                model_name: "Gemini 3.1 Pro (Low)".to_string(),
                input_price: 2.00,
                cache_input_price: 0.20,
                output_price: 12.00,
            },
        ]
    }

    #[test]
    fn parse_threshold_rule_supports_variable_boundaries_and_legacy_suffix() {
        let (short_base, short) = parse_threshold_rule("Gemini 3.1 Pro (Low) (< 200K)");
        assert_eq!(short_base, "gemini31prolow");
        assert_eq!(
            short,
            Some(ThresholdRule {
                is_greater: false,
                threshold_tokens: 200_000,
            })
        );

        let (long_base, long) = parse_threshold_rule("GPT-5.5 (>272k context length)");
        assert_eq!(long_base, "gpt55");
        assert_eq!(
            long,
            Some(ThresholdRule {
                is_greater: true,
                threshold_tokens: 272_000,
            })
        );
    }

    #[test]
    fn threshold_uses_prompt_tokens_without_counting_output() {
        let rules = gemini_pro_rules();

        // The prompt is 190k, so the 20k output must not move it to the >200k tier.
        let cost = calculate_cost(&rules, "Gemini 3.1 Pro (Low)", 190_000, 20_000, 0).unwrap();
        let expected = (190_000.0 / 1_000_000.0) * 2.00 + (20_000.0 / 1_000_000.0) * 12.00;
        assert!((cost - expected).abs() < 1e-12);
    }

    #[test]
    fn cache_read_tokens_count_toward_prompt_threshold() {
        let rules = gemini_pro_rules();

        // 190k input + 11k cached prompt = 201k prompt tokens, so the long tier applies.
        let cost = calculate_cost(&rules, "Gemini 3.1 Pro (Low)", 190_000, 20_000, 11_000).unwrap();
        let expected = (190_000.0 / 1_000_000.0) * 4.00
            + (11_000.0 / 1_000_000.0) * 0.40
            + (20_000.0 / 1_000_000.0) * 18.00;
        assert!((cost - expected).abs() < 1e-12);
    }

    #[test]
    fn threshold_boundary_uses_short_tier() {
        let rules = gemini_pro_rules();

        // Exactly 200k prompt tokens remains in the <=200k tier, even with output.
        let cost = calculate_cost(&rules, "Gemini 3.1 Pro (Low)", 200_000, 20_000, 0).unwrap();
        let expected = (200_000.0 / 1_000_000.0) * 2.00 + (20_000.0 / 1_000_000.0) * 12.00;
        assert!((cost - expected).abs() < 1e-12);
    }

    #[test]
    fn threshold_rule_wins_when_default_is_listed_first() {
        let rules = [
            PricingRule {
                model_name: "GPT-5.5".to_string(),
                input_price: 5.00,
                cache_input_price: 0.50,
                output_price: 30.00,
            },
            PricingRule {
                model_name: "GPT-5.5 (>272k context length)".to_string(),
                input_price: 10.00,
                cache_input_price: 1.00,
                output_price: 45.00,
            },
            PricingRule {
                model_name: "GPT-5.5 (<272k context length)".to_string(),
                input_price: 5.00,
                cache_input_price: 0.50,
                output_price: 30.00,
            },
        ];

        let short = calculate_cost(&rules, "GPT-5.5", 100_000, 20_000, 0).unwrap();
        let short_expected = (100_000.0 / 1_000_000.0) * 5.00 + (20_000.0 / 1_000_000.0) * 30.00;
        assert!((short - short_expected).abs() < 1e-12);

        let long = calculate_cost(&rules, "GPT-5.5", 300_000, 20_000, 0).unwrap();
        let long_expected = (300_000.0 / 1_000_000.0) * 10.00 + (20_000.0 / 1_000_000.0) * 45.00;
        assert!((long - long_expected).abs() < 1e-12);
    }
}
