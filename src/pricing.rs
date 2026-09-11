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

/// 載入價格規則並完成一次性的標籤解析，供大量聚合的 API 重複使用。
pub fn load_prepared_pricing_rules() -> PreparedPricingRules {
    PreparedPricingRules::from_rules(load_pricing_rules())
}

/// Parsed long-context threshold marker from a pricing rule label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ThresholdRule {
    is_greater: bool,
    threshold_tokens: u64,
}

/// Parse a model/rule label into a normalized base name and optional threshold.
///
/// Threshold labels have appeared as `(>272k)`, `(>272k context length)`, and
/// `(>200k)`. Parse the number instead of coupling matching to one boundary.
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

    let normalized = cleaned.chars().filter(|c| c.is_alphanumeric()).collect();
    (normalized, threshold)
}

fn threshold_matches(rule: ThresholdRule, prompt_tokens: u64) -> bool {
    if rule.is_greater {
        prompt_tokens > rule.threshold_tokens
    } else {
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

#[allow(dead_code)]
pub fn normalize_model_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// 已解析的單筆價格規則：把模型標籤的閾值解析與 Claude 判定提前算好，
/// 讓每筆 usage 的計價不必重複做字串剖析。
#[derive(Debug, Clone)]
struct PreparedRule {
    base: String,
    threshold: Option<ThresholdRule>,
    is_claude: bool,
}

/// 預先解析完成的價格規則集。
///
/// `find_pricing_rule` 原本每比對一個模型就對所有規則重新做一次
/// `parse_threshold_rule`（每次都有多次字串配置），在數萬筆 usage 的
/// 月報聚合下會佔掉絕大多數 CPU 時間；此型別把解析成本降為一次。
#[derive(Debug, Clone)]
pub struct PreparedPricingRules {
    rules: Vec<PricingRule>,
    parsed: Vec<PreparedRule>,
}

impl PreparedPricingRules {
    pub fn from_rules(rules: Vec<PricingRule>) -> Self {
        let parsed = rules
            .iter()
            .map(|rule| {
                let (base, threshold) = parse_threshold_rule(&rule.model_name);
                let is_claude = rule.model_name.to_ascii_lowercase().contains("claude");
                PreparedRule {
                    base,
                    threshold,
                    is_claude,
                }
            })
            .collect();
        Self { rules, parsed }
    }

    /// 與自由函式 `calculate_usage_cost` 相同的參數與錯誤語意，但走預先解析的規則。
    #[allow(clippy::too_many_arguments)]
    pub fn calculate_usage_cost(
        &self,
        model_name: Option<&str>,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write_5m: u64,
        cache_write_1h: u64,
    ) -> Result<f64, String> {
        if input == 0
            && output == 0
            && cache_read == 0
            && cache_write_5m == 0
            && cache_write_1h == 0
        {
            return Ok(0.0);
        }

        let model_name = model_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "缺少模型名稱，無法估算成本".to_string())?;
        self.calculate_cost(
            model_name,
            input,
            output,
            cache_read,
            cache_write_5m,
            cache_write_1h,
        )
    }

    fn find_rule_index(
        &self,
        model_base: &str,
        prompt_tokens: u64,
        contains_match: bool,
    ) -> Option<usize> {
        let mut best_rule = None;
        let mut best_base_len = 0;
        let mut best_has_threshold = false;

        for (index, prepared) in self.parsed.iter().enumerate() {
            if !rule_applies_to_context(
                &prepared.base,
                prepared.threshold,
                model_base,
                prompt_tokens,
                contains_match,
            ) {
                continue;
            }

            let base_len = prepared.base.len();
            let has_threshold = prepared.threshold.is_some();
            let is_more_specific = base_len > best_base_len;
            let is_same_base_with_threshold =
                base_len == best_base_len && has_threshold && !best_has_threshold;
            if best_rule.is_none() || is_more_specific || is_same_base_with_threshold {
                best_rule = Some(index);
                best_base_len = base_len;
                best_has_threshold = has_threshold;
            }
        }

        best_rule
    }

    /// 與 `calculate_cost` 相同的計價邏輯，但規則標籤只解析一次。
    pub fn calculate_cost(
        &self,
        model_name: &str,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write_5m: u64,
        cache_write_1h: u64,
    ) -> Result<f64, String> {
        if input == 0
            && output == 0
            && cache_read == 0
            && cache_write_5m == 0
            && cache_write_1h == 0
        {
            return Ok(0.0);
        }

        let (m_base, _) = parse_threshold_rule(model_name);
        if m_base.is_empty() {
            return Err(format!(
                "模型名稱為空，無法估算成本。來源模型：{}",
                model_name
            ));
        }

        let is_claude_model = self.parsed.iter().any(|prepared| {
            !prepared.base.is_empty()
                && (prepared.base == m_base
                    || m_base.contains(&prepared.base)
                    || prepared.base.contains(&m_base))
                && prepared.is_claude
        });
        let (priced_cache_write_5m, priced_cache_write_1h) = if is_claude_model {
            (cache_write_5m, cache_write_1h)
        } else {
            (0, 0)
        };
        // Provider long-context tiers apply to prompt tokens. Output tokens do not
        // increase the prompt size; cached reads and Claude cache writes do.
        let prompt_tokens = input
            .saturating_add(cache_read)
            .saturating_add(priced_cache_write_5m)
            .saturating_add(priced_cache_write_1h);

        // 1. Exact base name match (threshold-aware)
        let matched_index = self
            .find_rule_index(&m_base, prompt_tokens, false)
            // 2. Fallback: contains base name match
            .or_else(|| self.find_rule_index(&m_base, prompt_tokens, true));

        if let Some(index) = matched_index {
            let r = &self.rules[index];
            let input_cost = (input as f64 / 1_000_000.0) * r.input_price;
            let cache_cost = (cache_read as f64 / 1_000_000.0) * r.cache_input_price;
            let cache_write_5m_cost =
                (priced_cache_write_5m as f64 / 1_000_000.0) * r.input_price * 1.25;
            let cache_write_1h_cost =
                (priced_cache_write_1h as f64 / 1_000_000.0) * r.input_price * 2.0;
            let output_cost = (output as f64 / 1_000_000.0) * r.output_price;
            Ok(input_cost + cache_cost + cache_write_5m_cost + cache_write_1h_cost + output_cost)
        } else {
            Err(format!("找不到可用的模型價格規則：{}", model_name))
        }
    }
}

/// 便利建構子：直接由規則清單建立。
impl From<Vec<PricingRule>> for PreparedPricingRules {
    fn from(rules: Vec<PricingRule>) -> Self {
        Self::from_rules(rules)
    }
}

/// 測試用相容介面：直接以 `&[PricingRule]` 計價（每次呼叫都會解析規則，
/// 正式程式碼請改用 `PreparedPricingRules`）。
#[cfg(test)]
fn calculate_cost(
    rules: &[PricingRule],
    model_name: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write_5m: u64,
    cache_write_1h: u64,
) -> Result<f64, String> {
    PreparedPricingRules::from_rules(rules.to_vec()).calculate_cost(
        model_name,
        input,
        output,
        cache_read,
        cache_write_5m,
        cache_write_1h,
    )
}

#[cfg(test)]
fn calculate_usage_cost(
    rules: &[PricingRule],
    model_name: Option<&str>,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write_5m: u64,
    cache_write_1h: u64,
) -> Result<f64, String> {
    if input == 0 && output == 0 && cache_read == 0 && cache_write_5m == 0 && cache_write_1h == 0 {
        return Ok(0.0);
    }

    let model_name = model_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "缺少模型名稱，無法估算成本".to_string())?;
    calculate_cost(
        rules,
        model_name,
        input,
        output,
        cache_read,
        cache_write_5m,
        cache_write_1h,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_token_usage_without_model_costs_zero() {
        let cost = calculate_usage_cost(&[], None, 0, 0, 0, 0, 0).unwrap();
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn token_usage_without_model_reports_missing_metadata() {
        let error = calculate_usage_cost(&[], None, 10, 2, 3, 0, 0).unwrap_err();
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
            0,
            0,
        )
        .unwrap();

        assert!((cost - 0.068_139_3).abs() < f64::EPSILON);
    }

    #[test]
    fn claude_opus_5_variants_use_packaged_pricing() {
        let rules = load_pricing_rules();

        for model_name in [
            "claude-opus-5",
            "Claude Opus 5",
            "claude-opus-5-1m · high",
            "opus-5",
        ] {
            let cost = calculate_usage_cost(
                &rules,
                Some(model_name),
                1_000_000,
                1_000_000,
                1_000_000,
                0,
                0,
            )
            .unwrap();

            assert!(
                (cost - 30.5).abs() < 1e-9,
                "unexpected Opus 5 cost for {model_name}: {cost}"
            );
        }
    }

    #[test]
    fn glm_5_3_flash_uses_packaged_pricing() {
        let rules = load_pricing_rules();

        for model_name in ["glm-5.3-flash", "glm-5.3-flash:cloud", "GLM-5.3-Flash"] {
            let cost =
                calculate_usage_cost(&rules, Some(model_name), 1_000_000, 0, 1_000_000, 0, 0)
                    .unwrap();

            assert!(
                (cost - 0.18).abs() < 1e-9,
                "unexpected glm-5.3-flash cost for {model_name}: {cost}"
            );
        }
    }

    #[test]
    fn gpt_6_astra_context_tiers_use_packaged_pricing() {
        let rules = load_pricing_rules();

        // 1. Short context (prompt tokens <= 272k): $10.00 / $1.00 / $50.00 per 1M tokens
        for model_name in [
            "GPT-6 Astra",
            "GPT-6 Astra (<272k)",
            "gpt-6-astra",
            "GPT-6-Astra",
        ] {
            let cost =
                calculate_usage_cost(&rules, Some(model_name), 100_000, 50_000, 100_000, 0, 0)
                    .unwrap();

            let expected = (100_000.0 / 1_000_000.0) * 10.00
                + (100_000.0 / 1_000_000.0) * 1.00
                + (50_000.0 / 1_000_000.0) * 50.00;
            assert!(
                (cost - expected).abs() < 1e-9,
                "unexpected short-context cost for {model_name}: {cost}"
            );
        }

        // 2. Long context (prompt tokens > 272k): $20.00 / $2.00 / $75.00 per 1M tokens
        for model_name in [
            "GPT-6 Astra",
            "GPT-6 Astra (>272k)",
            "gpt-6-astra",
            "GPT-6-Astra",
        ] {
            let cost =
                calculate_usage_cost(&rules, Some(model_name), 300_000, 50_000, 0, 0, 0).unwrap();

            let expected = (300_000.0 / 1_000_000.0) * 20.00 + (50_000.0 / 1_000_000.0) * 75.00;
            assert!(
                (cost - expected).abs() < 1e-9,
                "unexpected long-context cost for {model_name}: {cost}"
            );
        }

        // 3. Regression test for session 01a074ab-f530-7cf1-b65f-d2e52a330258 turn 518
        // input: 1,021, output: 1,388, cache_read: 197,760 -> prompt = 198,781 tokens (<= 272k)
        let turn_518_cost =
            calculate_usage_cost(&rules, Some("gpt-6-astra"), 1_021, 1_388, 197_760, 0, 0)
                .expect("gpt-6-astra turn 518 cost should calculate successfully");
        let expected_turn_518 = (1_021.0 / 1_000_000.0) * 10.00
            + (197_760.0 / 1_000_000.0) * 1.00
            + (1_388.0 / 1_000_000.0) * 50.00;
        assert!((turn_518_cost - expected_turn_518).abs() < 1e-9);
    }

    #[test]
    fn gpt_daybreak_blue_uses_packaged_pricing() {
        let rules = load_pricing_rules();

        // Identical to gpt-5.6-sol: input 5.00, cache_read 0.50, output 30.00
        for model_name in [
            "gpt-daybreak-blue-latest",
            "gpt-daybreak-blue",
            "GPT-Daybreak-Blue-Latest",
            "GPT-Daybreak-Blue",
        ] {
            let cost = calculate_usage_cost(
                &rules,
                Some(model_name),
                1_000_000,
                1_000_000,
                1_000_000,
                0,
                0,
            )
            .unwrap();

            // 5.00 + 0.50 + 30.00 = 35.50
            assert!(
                (cost - 35.5).abs() < 1e-9,
                "unexpected cost for {model_name}: {cost}"
            );
        }

        // Regression test for session 01a06d15-a222-7bc2-9c0b-19ec24de2c80 turn 6
        // input: 227, output: 180, cache_read: 42,886
        let turn_6_cost = calculate_usage_cost(
            &rules,
            Some("gpt-daybreak-blue-latest"),
            227,
            180,
            42_886,
            0,
            0,
        )
        .expect("gpt-daybreak-blue-latest turn 6 cost should calculate successfully");
        let expected_turn_6 = (227.0 / 1_000_000.0) * 5.00
            + (42_886.0 / 1_000_000.0) * 0.50
            + (180.0 / 1_000_000.0) * 30.00;
        assert!((turn_6_cost - expected_turn_6).abs() < 1e-9);
    }

    #[test]
    fn gpt_reserve_matches_gpt_5_6_luna_packaged_pricing() {
        let rules = load_pricing_rules();

        // gpt-reserve is an alias of gpt-5.6-luna: input 1.00, cache read 0.10, output 6.00
        let luna = calculate_usage_cost(
            &rules,
            Some("gpt-5.6-luna"),
            1_000_000,
            1_000_000,
            1_000_000,
            0,
            0,
        )
        .expect("gpt-5.6-luna should have a pricing rule");

        for model_name in ["gpt-reserve", "GPT-Reserve", "gpt-reserve · high"] {
            let cost = calculate_usage_cost(
                &rules,
                Some(model_name),
                1_000_000,
                1_000_000,
                1_000_000,
                0,
                0,
            )
            .unwrap_or_else(|error| panic!("{model_name} should have a pricing rule: {error}"));

            assert!(
                (cost - luna).abs() < 1e-9,
                "unexpected gpt-reserve cost for {model_name}: {cost}"
            );
        }

        // Regression test for session 01a074ab-f530-7cf1-b65f-d2e52a330258 turn 951
        let turn_951_cost =
            calculate_usage_cost(&rules, Some("gpt-reserve"), 1_021, 1_388, 197_760, 0, 0)
                .expect("gpt-reserve turn 951 cost should calculate successfully");
        let expected_turn_951 = (1_021.0 / 1_000_000.0) * 1.00
            + (197_760.0 / 1_000_000.0) * 0.10
            + (1_388.0 / 1_000_000.0) * 6.00;
        assert!((turn_951_cost - expected_turn_951).abs() < 1e-9);
    }

    #[test]
    fn mai_code_1_1_flash_resolves_pricing_from_csv() {
        let rules = load_pricing_rules();

        for model_name in [
            "mai-code-1.1-flash",
            "MAI-Code-1.1-Flash",
            "mai-code-1.1-flash-picker · medium",
        ] {
            let cost = calculate_usage_cost(
                &rules,
                Some(model_name),
                1_000_000,
                1_000_000,
                1_000_000,
                0,
                0,
            )
            .unwrap_or_else(|error| panic!("{model_name} should have a pricing rule: {error}"));

            // input 0.75 + cache read 0.075 + output 4.50 = 5.325
            assert!(
                (cost - 5.325).abs() < 1e-9,
                "unexpected MAI-Code-1.1-Flash cost for {model_name}: {cost}"
            );
        }

        // The 1.1 rule must not shadow the 1.0 rule (or vice versa).
        let flash_1 =
            calculate_usage_cost(&rules, Some("mai-code-1-flash"), 1_000_000, 0, 0, 0, 0).unwrap();
        assert!((flash_1 - 0.75).abs() < 1e-9);
    }

    #[test]
    fn gemini_3_8_flash_thinking_levels_use_packaged_pricing() {
        let rules = load_pricing_rules();

        for model_name in [
            "Gemini 3.8 Flash",
            "Gemini 3.8 Flash (Medium)",
            "Gemini 3.8 Flash (High)",
            "Gemini 3.8 Flash (Low)",
            "gemini-3.8-flash",
        ] {
            let cost = calculate_usage_cost(
                &rules,
                Some(model_name),
                1_000_000,
                1_000_000,
                1_000_000,
                0,
                0,
            )
            .unwrap();

            assert!(
                (cost - 9.15).abs() < 1e-9,
                "unexpected Gemini 3.8 Flash cost for {model_name}: {cost}"
            );
        }

        let turn_306_cost = calculate_usage_cost(
            &rules,
            Some("Gemini 3.8 Flash (High)"),
            142_420,
            76_059,
            129_867,
            0,
            0,
        )
        .expect("Gemini 3.8 Flash (High) turn 306 cost should calculate successfully");
        let expected = (142_420.0 / 1_000_000.0) * 1.50
            + (129_867.0 / 1_000_000.0) * 0.15
            + (76_059.0 / 1_000_000.0) * 7.50;
        assert!((turn_306_cost - expected).abs() < 1e-9);
    }

    #[test]
    fn gemini_3_7_flash_thinking_levels_use_packaged_pricing() {
        let rules = load_pricing_rules();

        for model_name in ["Gemini 3.7 Flash (High)", "Gemini 3.7 Flash (Low)"] {
            let cost = calculate_usage_cost(
                &rules,
                Some(model_name),
                1_000_000,
                1_000_000,
                1_000_000,
                0,
                0,
            )
            .unwrap();

            assert!(
                (cost - 9.15).abs() < 1e-9,
                "unexpected Gemini 3.7 Flash cost for {model_name}: {cost}"
            );
        }
    }

    #[test]
    fn gemini_3_6_flash_thinking_levels_use_packaged_pricing() {
        let rules = load_pricing_rules();

        for model_name in ["Gemini 3.6 Flash (High)", "Gemini 3.6 Flash (Low)"] {
            let cost = calculate_usage_cost(
                &rules,
                Some(model_name),
                1_000_000,
                1_000_000,
                1_000_000,
                0,
                0,
            )
            .unwrap();

            assert!(
                (cost - 9.15).abs() < 1e-9,
                "unexpected Gemini 3.6 Flash cost for {model_name}: {cost}"
            );
        }
    }

    #[test]
    fn cache_writes_use_official_ttl_multipliers() {
        let rules = [PricingRule {
            model_name: "Claude Fable 5".to_string(),
            input_price: 10.0,
            cache_input_price: 1.0,
            output_price: 50.0,
        }];

        let cost = calculate_usage_cost(
            &rules,
            Some("claude-fable-5"),
            1_000_000,
            1_000_000,
            1_000_000,
            1_000_000,
            1_000_000,
        )
        .unwrap();

        assert!((cost - 93.5).abs() < 1e-9);
    }

    #[test]
    fn cache_write_ttl_fields_do_not_change_non_claude_cost() {
        let rules = [PricingRule {
            model_name: "GPT-5".to_string(),
            input_price: 2.0,
            cache_input_price: 0.2,
            output_price: 8.0,
        }];

        let cost = calculate_usage_cost(
            &rules,
            Some("gpt-5"),
            1_000_000,
            1_000_000,
            1_000_000,
            1_000_000,
            1_000_000,
        )
        .unwrap();

        assert!((cost - 10.2).abs() < 1e-9);
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

        // A 190k prompt remains in the short tier even with a 20k response.
        let cost =
            calculate_cost(&rules, "Gemini 3.1 Pro (Low)", 190_000, 20_000, 0, 0, 0).unwrap();
        let expected = (190_000.0 / 1_000_000.0) * 2.00 + (20_000.0 / 1_000_000.0) * 12.00;
        assert!((cost - expected).abs() < 1e-12);
    }

    #[test]
    fn cache_read_tokens_count_toward_prompt_threshold() {
        let rules = gemini_pro_rules();

        // 190k input + 11k cached prompt = 201k, so the long tier applies.
        let cost = calculate_cost(
            &rules,
            "Gemini 3.1 Pro (Low)",
            190_000,
            20_000,
            11_000,
            0,
            0,
        )
        .unwrap();
        let expected = (190_000.0 / 1_000_000.0) * 4.00
            + (11_000.0 / 1_000_000.0) * 0.40
            + (20_000.0 / 1_000_000.0) * 18.00;
        assert!((cost - expected).abs() < 1e-12);
    }

    #[test]
    fn threshold_boundary_uses_short_tier() {
        let rules = gemini_pro_rules();

        // Exactly 200k prompt tokens remains in the <=200k tier.
        let cost =
            calculate_cost(&rules, "Gemini 3.1 Pro (Low)", 200_000, 20_000, 0, 0, 0).unwrap();
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

        let short = calculate_cost(&rules, "GPT-5.5", 100_000, 20_000, 0, 0, 0).unwrap();
        let short_expected = (100_000.0 / 1_000_000.0) * 5.00 + (20_000.0 / 1_000_000.0) * 30.00;
        assert!((short - short_expected).abs() < 1e-12);

        let long = calculate_cost(&rules, "GPT-5.5", 300_000, 20_000, 0, 0, 0).unwrap();
        let long_expected = (300_000.0 / 1_000_000.0) * 10.00 + (20_000.0 / 1_000_000.0) * 45.00;
        assert!((long - long_expected).abs() < 1e-12);
    }

    #[test]
    fn contains_fallback_prefers_the_most_specific_model_base() {
        let rules = [
            PricingRule {
                model_name: "GPT-5.4 (<272k)".to_string(),
                input_price: 2.50,
                cache_input_price: 0.25,
                output_price: 15.00,
            },
            PricingRule {
                model_name: "GPT-5.4-mini".to_string(),
                input_price: 0.75,
                cache_input_price: 0.08,
                output_price: 4.50,
            },
        ];

        let cost = calculate_cost(&rules, "GPT-5.4-mini-picker", 100_000, 0, 0, 0, 0).unwrap();

        assert!((cost - 0.075).abs() < 1e-12);
    }

    #[test]
    fn packaged_gemini_pricing_uses_standard_cache_rates() {
        let rules = load_pricing_rules();

        let short_context =
            calculate_cost(&rules, "Gemini 3.1 Pro (Low)", 100_000, 0, 100_000, 0, 0).unwrap();
        assert!((short_context - 0.22).abs() < 1e-12);

        let long_context =
            calculate_cost(&rules, "Gemini 3.1 Pro (Low)", 201_000, 0, 0, 0, 0).unwrap();
        assert!((long_context - 0.804).abs() < 1e-12);
    }

    #[test]
    fn packaged_grok_build_01_pricing_is_distinct_from_grok_45() {
        let rules = load_pricing_rules();

        let short_context =
            calculate_usage_cost(&rules, Some("grok-build-0.1"), 100_000, 0, 100_000, 0, 0)
                .unwrap();
        let long_context =
            calculate_usage_cost(&rules, Some("grok-build-0.1"), 201_000, 0, 0, 0, 0).unwrap();
        let grok_45 =
            calculate_usage_cost(&rules, Some("grok-4.5"), 100_000, 0, 100_000, 0, 0).unwrap();

        assert!((short_context - 0.12).abs() < 1e-12);
        assert!((long_context - 0.402).abs() < 1e-12);
        assert!((grok_45 - 0.23).abs() < 1e-12);
    }

    #[test]
    fn packaged_grok_46_pricing_uses_context_tiers_for_each_reasoning_effort() {
        let rules = load_pricing_rules();

        for model_name in [
            "grok-4.6",
            "Grok 4.6 (Low)",
            "Grok 4.6 (Medium)",
            "Grok 4.6 (High)",
        ] {
            let short_context =
                calculate_usage_cost(&rules, Some(model_name), 100_000, 1_000_000, 100_000, 0, 0)
                    .unwrap();
            let long_context =
                calculate_usage_cost(&rules, Some(model_name), 201_000, 1_000_000, 0, 0, 0)
                    .unwrap();

            assert!(
                (short_context - 6.25).abs() < 1e-12,
                "unexpected short-context cost for {model_name}: {short_context}"
            );
            assert!(
                (long_context - 12.804).abs() < 1e-12,
                "unexpected long-context cost for {model_name}: {long_context}"
            );
        }
    }

    #[test]
    fn kimi_k3_resolves_pricing_from_csv() {
        let rules = load_pricing_rules();

        let cost = calculate_usage_cost(&rules, Some("kimi-k3"), 1_000_000, 1_000_000, 0, 0, 0)
            .expect("kimi-k3 should have a pricing rule");
        // input 3.00 + output 15.00 = 18.00
        assert!((cost - 18.0).abs() < 1e-12);
    }

    #[test]
    fn composer_2_5_speed_tiers_resolve_distinct_pricing_from_csv() {
        let rules = load_pricing_rules();

        let standard = calculate_usage_cost(
            &rules,
            Some("composer-2.5"),
            1_000_000,
            1_000_000,
            1_000_000,
            0,
            0,
        )
        .expect("composer-2.5 should have a pricing rule");
        // input 0.50 + cache read 0.20 + output 2.50 = 3.20
        assert!((standard - 3.2).abs() < 1e-12);

        let fast = calculate_usage_cost(
            &rules,
            Some("composer-2.5-fast"),
            1_000_000,
            1_000_000,
            1_000_000,
            0,
            0,
        )
        .expect("composer-2.5-fast should have a pricing rule");
        // input 3.00 + cache read 0.50 + output 15.00 = 18.50
        assert!((fast - 18.5).abs() < 1e-12);
    }

    #[test]
    fn muse_spark_1_3_and_contributor_resolve_pricing_from_csv() {
        let rules = load_pricing_rules();

        let standard = calculate_usage_cost(
            &rules,
            Some("muse-spark-1.3"),
            1_000_000,
            1_000_000,
            1_000_000,
            0,
            0,
        )
        .expect("muse-spark-1.3 should have a pricing rule");
        // input 1.25 + cache read 0.15 + output 4.25 = 5.65
        assert!((standard - 5.65).abs() < 1e-12);

        let contributor = calculate_usage_cost(
            &rules,
            Some("muse-spark-1.3-contributor"),
            1_000_000,
            1_000_000,
            1_000_000,
            0,
            0,
        )
        .expect("muse-spark-1.3-contributor should have a pricing rule");
        // input 0.10 + cache read 0.002 + output 0.20 = 0.302
        assert!((contributor - 0.302).abs() < 1e-12);

        let standard_12 = calculate_usage_cost(
            &rules,
            Some("muse-spark-1.2"),
            1_000_000,
            1_000_000,
            1_000_000,
            0,
            0,
        )
        .expect("muse-spark-1.2 should have a pricing rule");
        assert!((standard_12 - 5.65).abs() < 1e-12);

        let contributor_12 = calculate_usage_cost(
            &rules,
            Some("muse-spark-1.2-contributor"),
            1_000_000,
            1_000_000,
            1_000_000,
            0,
            0,
        )
        .expect("muse-spark-1.2-contributor should have a pricing rule");
        assert!((contributor_12 - 0.302).abs() < 1e-12);
    }
}
