use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

pub const LITELLM_PRICING_FILENAME: &str = "model_prices_and_context_window.json";
pub const LITELLM_REMOTE_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// 內建 LiteLLM 模型定價 JSON 資料，確保在獨立執行檔或未攜帶外部資原始檔的情況下
/// 依然具備完整 4,200+ 模型之官方價格規則（包含 gemini-3.8-flash 等）。
pub const PACKAGED_LITELLM_JSON: &str = include_str!("../model_prices_and_context_window.json");

#[derive(Debug, Clone)]
pub struct PricingRule {
    pub model_name: String,
    pub input_price: f64,
    pub cache_input_price: f64,
    pub output_price: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingEntry {
    pub model_name: String,
    pub deployment_type: String,
    pub unit: String,
    pub input_price: f64,
    pub cache_input_price: f64,
    pub output_price: f64,
    pub batch_api_price: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PricingUpdateResult {
    pub success: bool,
    pub models_count: usize,
    pub file_path: String,
    pub message: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct LiteLlmModelEntry {
    #[serde(default)]
    pub litellm_provider: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub input_cost_per_token: Option<f64>,
    #[serde(default)]
    pub output_cost_per_token: Option<f64>,
    #[serde(default)]
    pub cache_read_input_token_cost: Option<f64>,
    #[serde(default)]
    pub cache_creation_input_token_cost: Option<f64>,
    #[serde(default)]
    pub output_cost_per_reasoning_token: Option<f64>,

    // Tiered pricing fields
    #[serde(default)]
    pub input_cost_per_token_above_100k_tokens: Option<f64>,
    #[serde(default)]
    pub input_cost_per_token_above_128k_tokens: Option<f64>,
    #[serde(default)]
    pub input_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(default)]
    pub input_cost_per_token_above_272k_tokens: Option<f64>,

    #[serde(default)]
    pub output_cost_per_token_above_100k_tokens: Option<f64>,
    #[serde(default)]
    pub output_cost_per_token_above_128k_tokens: Option<f64>,
    #[serde(default)]
    pub output_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(default)]
    pub output_cost_per_token_above_272k_tokens: Option<f64>,

    #[serde(default)]
    pub cache_read_input_token_cost_above_100k_tokens: Option<f64>,
    #[serde(default)]
    pub cache_read_input_token_cost_above_200k_tokens: Option<f64>,
    #[serde(default)]
    pub cache_read_input_token_cost_above_272k_tokens: Option<f64>,

    // Batch pricing
    #[serde(default)]
    pub input_cost_per_token_batches: Option<f64>,
    #[serde(default)]
    pub output_cost_per_token_batches: Option<f64>,
}

fn fallback_pricing_entries() -> Vec<PricingEntry> {
    vec![
        PricingEntry {
            model_name: "Gemini 3.8 Flash".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 0.75,
            cache_input_price: 0.075,
            output_price: 3.75,
            batch_api_price: "0.38/1.88".to_string(),
        },
        PricingEntry {
            model_name: "gemini-3.8-flash".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 0.75,
            cache_input_price: 0.075,
            output_price: 3.75,
            batch_api_price: "0.38/1.88".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 3.7 Flash".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 0.75,
            cache_input_price: 0.075,
            output_price: 3.75,
            batch_api_price: "0.38/1.88".to_string(),
        },
        PricingEntry {
            model_name: "gemini-3.7-flash".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 0.75,
            cache_input_price: 0.075,
            output_price: 3.75,
            batch_api_price: "0.38/1.88".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 4 Argon".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 2.00,
            cache_input_price: 0.10,
            output_price: 10.00,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "gemini-4-argon".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 2.00,
            cache_input_price: 0.10,
            output_price: 10.00,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 3.5 Flash".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 1.50,
            cache_input_price: 0.375,
            output_price: 9.00,
            batch_api_price: "0.75/0.1875/4.50".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 1.5 Flash".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 0.075,
            cache_input_price: 0.01875,
            output_price: 0.30,
            batch_api_price: "0.0375/0.009375/0.15".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 1.5 Pro".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 1.25,
            cache_input_price: 0.3125,
            output_price: 5.00,
            batch_api_price: "0.625/0.15625/2.50".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 2.0 Flash".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 0.10,
            cache_input_price: 0.025,
            output_price: 0.40,
            batch_api_price: "0.05/0.0125/0.20".to_string(),
        },
    ]
}

fn supplemental_pricing_entries() -> Vec<PricingEntry> {
    vec![
        PricingEntry {
            model_name: "composer-2.5".to_string(),
            deployment_type: "Cursor".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 0.50,
            cache_input_price: 0.20,
            output_price: 2.50,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "composer-2.5-fast".to_string(),
            deployment_type: "Cursor".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 3.00,
            cache_input_price: 0.50,
            output_price: 15.00,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "cursor-small".to_string(),
            deployment_type: "Cursor".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 0.10,
            cache_input_price: 0.0,
            output_price: 0.10,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "Cursor Agent".to_string(),
            deployment_type: "Cursor".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 3.00,
            cache_input_price: 0.30,
            output_price: 15.00,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "Codex Auto Review".to_string(),
            deployment_type: "Global".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 0.10,
            cache_input_price: 0.0,
            output_price: 0.10,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "Claude Opus 5.5-fast".to_string(),
            deployment_type: "Global".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 8.00,
            cache_input_price: 0.40,
            output_price: 40.00,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "Claude Opus 4.8-fast".to_string(),
            deployment_type: "Global".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 10.00,
            cache_input_price: 1.00,
            output_price: 50.00,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "Claude Opus 4.7-fast".to_string(),
            deployment_type: "Global".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 10.00,
            cache_input_price: 1.00,
            output_price: 50.00,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "Claude Sonnet 4.6 (Thinking)".to_string(),
            deployment_type: "Global".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 3.00,
            cache_input_price: 0.30,
            output_price: 15.00,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "Claude Opus 4.6 (Thinking)".to_string(),
            deployment_type: "Global".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 5.00,
            cache_input_price: 0.50,
            output_price: 25.00,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "Claude Opus 4.6 (Thinking) (<200k)".to_string(),
            deployment_type: "Global".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 5.00,
            cache_input_price: 0.50,
            output_price: 25.00,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "Claude Opus 4.6 (Thinking) (>200k)".to_string(),
            deployment_type: "Global".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 10.00,
            cache_input_price: 1.00,
            output_price: 37.50,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "GPT-OSS 120B (Medium)".to_string(),
            deployment_type: "Local/OSS".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 0.0,
            cache_input_price: 0.0,
            output_price: 0.0,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "MAI-Code-1-Flash".to_string(),
            deployment_type: "Global".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 0.75,
            cache_input_price: 0.075,
            output_price: 4.50,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "MAI-Code-1.1-Flash".to_string(),
            deployment_type: "Global".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 0.75,
            cache_input_price: 0.075,
            output_price: 4.50,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "kimi-k3.1-pro".to_string(),
            deployment_type: "Global".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 2.00,
            cache_input_price: 0.20,
            output_price: 10.00,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "Kimi K3.1 Pro".to_string(),
            deployment_type: "Global".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 2.00,
            cache_input_price: 0.20,
            output_price: 10.00,
            batch_api_price: "N/A".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 4 Hydrogen".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 1.50,
            cache_input_price: 0.075,
            output_price: 7.50,
            batch_api_price: "0.75/0.0375/3.75".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 4 Hydrogen (Low)".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 1.50,
            cache_input_price: 0.075,
            output_price: 7.50,
            batch_api_price: "0.75/0.0375/3.75".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 4 Hydrogen (Medium)".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 1.50,
            cache_input_price: 0.075,
            output_price: 7.50,
            batch_api_price: "0.75/0.0375/3.75".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 4 Hydrogen (High)".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 1.50,
            cache_input_price: 0.075,
            output_price: 7.50,
            batch_api_price: "0.75/0.0375/3.75".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 4 Hydrogen (Extra)".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 1.50,
            cache_input_price: 0.075,
            output_price: 7.50,
            batch_api_price: "0.75/0.0375/3.75".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 4 Carbon".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 4.00,
            cache_input_price: 0.20,
            output_price: 20.00,
            batch_api_price: "2.00/0.10/10.00".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 4 Carbon (Medium)".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 4.00,
            cache_input_price: 0.20,
            output_price: 20.00,
            batch_api_price: "2.00/0.10/10.00".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 4 Carbon (High)".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 4.00,
            cache_input_price: 0.20,
            output_price: 20.00,
            batch_api_price: "2.00/0.10/10.00".to_string(),
        },
        PricingEntry {
            model_name: "Gemini 4 Carbon (Low)".to_string(),
            deployment_type: "Google AI".to_string(),
            unit: "1M Tokens".to_string(),
            input_price: 4.00,
            cache_input_price: 0.20,
            output_price: 20.00,
            batch_api_price: "2.00/0.10/10.00".to_string(),
        },
    ]
}

/// 記錄下載快取時的程式版本，用來判斷快取是否仍可蓋過內建定價資料。
const LITELLM_CACHE_VERSION_FILENAME: &str = "model_prices_and_context_window.json.version";

/// 使用者以 `pricing update` 下載的快取只在「下載當時的程式版本」生效。
/// 升級程式後內建 JSON 可能收錄更新的模型，舊快取若繼續蓋過它會讓新模型找不到價格，
/// 因此版本不符（或沒有版本標記的舊快取）時回退到內建資料，需重新執行 `pricing update`。
fn pricing_cache_is_current(dir: &std::path::Path) -> bool {
    dir.join(LITELLM_PRICING_FILENAME).is_file()
        && std::fs::read_to_string(dir.join(LITELLM_CACHE_VERSION_FILENAME))
            .is_ok_and(|v| v.trim() == env!("CARGO_PKG_VERSION"))
}

pub fn resolve_pricing_file_path() -> PathBuf {
    let insights_dir = crate::db::get_insights_dir();
    let user_cache = insights_dir.join(LITELLM_PRICING_FILENAME);
    if pricing_cache_is_current(&insights_dir) {
        return user_cache;
    }

    if let Some(resource) = crate::paths::find_resource(LITELLM_PRICING_FILENAME) {
        return resource;
    }

    user_cache
}

fn is_native_provider_for_model(model_name: &str, provider: &str) -> bool {
    let lower_model = model_name.to_lowercase();
    let lower_provider = provider.to_lowercase();

    if lower_model.contains("grok") {
        return lower_provider == "xai";
    }
    if lower_model.contains("claude") {
        return lower_provider == "anthropic";
    }
    if lower_model.contains("gemini") {
        return lower_provider == "gemini"
            || lower_provider == "google"
            || lower_provider.starts_with("vertex_ai");
    }
    if lower_model.contains("gpt") || lower_model.starts_with("o1") || lower_model.starts_with("o3")
    {
        return lower_provider == "openai" || lower_provider == "chatgpt";
    }
    if lower_model.contains("kimi") {
        return lower_provider == "moonshot";
    }
    if lower_model.contains("glm") {
        return lower_provider == "zai" || lower_provider == "zhipu";
    }
    if lower_model.contains("deepseek") {
        return lower_provider == "deepseek" || lower_provider == "deepseek-ai";
    }
    if lower_model.contains("muse") {
        return lower_provider == "meta";
    }
    if lower_model.contains("minimax") || lower_model.contains("m2.") || lower_model.contains("m3")
    {
        return lower_provider == "minimax";
    }
    if lower_model.contains("mai-code") {
        return lower_provider == "github_copilot";
    }
    if lower_model.contains("mimo") {
        return lower_provider == "xiaomi_mimo" || lower_provider == "xiaomi";
    }

    false
}

fn calculate_provider_priority(model_name: &str, provider: &str) -> u8 {
    if is_native_provider_for_model(model_name, provider) {
        return 4;
    }
    match provider.to_lowercase().as_str() {
        "anthropic"
        | "openai"
        | "chatgpt"
        | "google"
        | "gemini"
        | "vertex_ai"
        | "vertex_ai-language-models"
        | "xai"
        | "moonshot"
        | "minimax"
        | "meta"
        | "deepseek"
        | "github_copilot"
        | "xiaomi_mimo"
        | "xiaomi" => 3,
        "cursor" | "global" | "local/oss" => 2,
        _ => 1,
    }
}

pub fn parse_litellm_json<R: std::io::Read>(reader: R) -> Result<Vec<PricingEntry>, String> {
    let raw_map: HashMap<String, LiteLlmModelEntry> =
        serde_json::from_reader(reader).map_err(|e| e.to_string())?;

    let mut direct_entries = Vec::with_capacity(raw_map.len());
    let mut alias_map: HashMap<String, (u8, PricingEntry)> = HashMap::new();

    // 排序 keys 以保證跨平台與測試的確定性
    let mut sorted_keys: Vec<_> = raw_map.keys().collect();
    sorted_keys.sort();

    for key in sorted_keys {
        if key == "sample_spec" || key == "fallback_generalizations" {
            continue;
        }

        let entry = &raw_map[key];
        if entry.input_cost_per_token.is_none() && entry.output_cost_per_token.is_none() {
            continue;
        }

        let provider = entry
            .litellm_provider
            .clone()
            .unwrap_or_else(|| "Unknown".to_string());
        let priority = calculate_provider_priority(key, &provider);

        let input_price = entry.input_cost_per_token.unwrap_or(0.0) * 1_000_000.0;
        let cache_price = entry.cache_read_input_token_cost.unwrap_or(0.0) * 1_000_000.0;
        let output_price = entry.output_cost_per_token.unwrap_or(0.0) * 1_000_000.0;

        let batch_price = match (
            entry.input_cost_per_token_batches,
            entry.output_cost_per_token_batches,
        ) {
            (Some(in_b), Some(out_b)) => format!("{:.2}/{:.2}", in_b * 1e6, out_b * 1e6),
            _ => "N/A".to_string(),
        };

        let pe = PricingEntry {
            model_name: key.clone(),
            deployment_type: provider.clone(),
            unit: "1M Tokens".to_string(),
            input_price,
            cache_input_price: cache_price,
            output_price,
            batch_api_price: batch_price.clone(),
        };

        direct_entries.push(pe);

        if key == "gpt-5.6-luna" {
            alias_map.insert(
                "gpt-reserve".to_string(),
                (
                    priority,
                    PricingEntry {
                        model_name: "gpt-reserve".to_string(),
                        deployment_type: provider.clone(),
                        unit: "1M Tokens".to_string(),
                        input_price,
                        cache_input_price: cache_price,
                        output_price,
                        batch_api_price: batch_price.clone(),
                    },
                ),
            );
        }

        // 階梯價格支援
        let tiers = [
            (
                "100k",
                entry.input_cost_per_token_above_100k_tokens,
                entry.cache_read_input_token_cost_above_100k_tokens,
                entry.output_cost_per_token_above_100k_tokens,
            ),
            (
                "128k",
                entry.input_cost_per_token_above_128k_tokens,
                None,
                entry.output_cost_per_token_above_128k_tokens,
            ),
            (
                "200k",
                entry.input_cost_per_token_above_200k_tokens,
                entry.cache_read_input_token_cost_above_200k_tokens,
                entry.output_cost_per_token_above_200k_tokens,
            ),
            (
                "272k",
                entry.input_cost_per_token_above_272k_tokens,
                entry.cache_read_input_token_cost_above_272k_tokens,
                entry.output_cost_per_token_above_272k_tokens,
            ),
        ];

        for (tier_name, in_above, cache_above, out_above) in tiers {
            if in_above.is_some() || out_above.is_some() || cache_above.is_some() {
                let tiered_in = in_above.map(|v| v * 1e6).unwrap_or(input_price);
                let tiered_cache = cache_above.map(|v| v * 1e6).unwrap_or(cache_price);
                let tiered_out = out_above.map(|v| v * 1e6).unwrap_or(output_price);

                direct_entries.push(PricingEntry {
                    model_name: format!("{key} (<{tier_name})"),
                    deployment_type: provider.clone(),
                    unit: "1M Tokens".to_string(),
                    input_price,
                    cache_input_price: cache_price,
                    output_price,
                    batch_api_price: batch_price.clone(),
                });

                direct_entries.push(PricingEntry {
                    model_name: format!("{key} (>{tier_name})"),
                    deployment_type: provider.clone(),
                    unit: "1M Tokens".to_string(),
                    input_price: tiered_in,
                    cache_input_price: tiered_cache,
                    output_price: tiered_out,
                    batch_api_price: batch_price.clone(),
                });

                if key == "gpt-5.6-luna" {
                    alias_map.insert(
                        format!("gpt-reserve (<{tier_name})"),
                        (
                            priority,
                            PricingEntry {
                                model_name: format!("gpt-reserve (<{tier_name})"),
                                deployment_type: provider.clone(),
                                unit: "1M Tokens".to_string(),
                                input_price,
                                cache_input_price: cache_price,
                                output_price,
                                batch_api_price: batch_price.clone(),
                            },
                        ),
                    );

                    alias_map.insert(
                        format!("gpt-reserve (>{tier_name})"),
                        (
                            priority,
                            PricingEntry {
                                model_name: format!("gpt-reserve (>{tier_name})"),
                                deployment_type: provider.clone(),
                                unit: "1M Tokens".to_string(),
                                input_price: tiered_in,
                                cache_input_price: tiered_cache,
                                output_price: tiered_out,
                                batch_api_price: batch_price.clone(),
                            },
                        ),
                    );
                }
            }
        }

        // 短別名登記：僅允許原廠 Provider（priority >= 3，如 "xai/grok-4.7" 或 "meta/muse-spark-1.3" 或 "moonshot/kimi-k3"）登記別名
        if priority >= 3 {
            if let Some((prefix, short_name)) = key.split_once('/') {
                if !short_name.is_empty() {
                    let lower_short = short_name.to_lowercase();
                    let should_insert = match alias_map.get(&lower_short) {
                        None => true,
                        Some((old_prio, _)) => priority > *old_prio,
                    };

                    if should_insert {
                        alias_map.insert(
                            lower_short.clone(),
                            (
                                priority,
                                PricingEntry {
                                    model_name: short_name.to_string(),
                                    deployment_type: provider.clone(),
                                    unit: "1M Tokens".to_string(),
                                    input_price,
                                    cache_input_price: cache_price,
                                    output_price,
                                    batch_api_price: batch_price.clone(),
                                },
                            ),
                        );

                        if prefix == "xiaomi_mimo" {
                            alias_map.insert(
                                format!("xiaomi/{lower_short}"),
                                (
                                    priority,
                                    PricingEntry {
                                        model_name: format!("xiaomi/{short_name}"),
                                        deployment_type: provider.clone(),
                                        unit: "1M Tokens".to_string(),
                                        input_price,
                                        cache_input_price: cache_price,
                                        output_price,
                                        batch_api_price: batch_price.clone(),
                                    },
                                ),
                            );
                        }

                        for (tier_name, in_above, cache_above, out_above) in tiers {
                            if in_above.is_some() || out_above.is_some() || cache_above.is_some() {
                                let tiered_in = in_above.map(|v| v * 1e6).unwrap_or(input_price);
                                let tiered_cache =
                                    cache_above.map(|v| v * 1e6).unwrap_or(cache_price);
                                let tiered_out = out_above.map(|v| v * 1e6).unwrap_or(output_price);

                                alias_map.insert(
                                    format!("{lower_short} (<{tier_name})"),
                                    (
                                        priority,
                                        PricingEntry {
                                            model_name: format!("{short_name} (<{tier_name})"),
                                            deployment_type: provider.clone(),
                                            unit: "1M Tokens".to_string(),
                                            input_price,
                                            cache_input_price: cache_price,
                                            output_price,
                                            batch_api_price: batch_price.clone(),
                                        },
                                    ),
                                );

                                alias_map.insert(
                                    format!("{lower_short} (>{tier_name})"),
                                    (
                                        priority,
                                        PricingEntry {
                                            model_name: format!("{short_name} (>{tier_name})"),
                                            deployment_type: provider.clone(),
                                            unit: "1M Tokens".to_string(),
                                            input_price: tiered_in,
                                            cache_input_price: tiered_cache,
                                            output_price: tiered_out,
                                            batch_api_price: batch_price.clone(),
                                        },
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // 依優先度由高到低加入別名，確保原廠排在最前面
    let mut alias_list: Vec<_> = alias_map.into_values().collect();
    alias_list.sort_by(|(p1, _), (p2, _)| p2.cmp(p1));
    for (_prio, pe) in alias_list {
        direct_entries.push(pe);
    }

    Ok(direct_entries)
}

pub fn load_pricing_entries() -> Vec<PricingEntry> {
    let file_path = resolve_pricing_file_path();
    let mut entries = if let Ok(file) = File::open(&file_path) {
        let reader = BufReader::new(file);
        parse_litellm_json(reader).unwrap_or_default()
    } else {
        Vec::new()
    };

    if entries.is_empty() {
        entries = parse_litellm_json(std::io::Cursor::new(PACKAGED_LITELLM_JSON.as_bytes()))
            .unwrap_or_default();
    }

    if entries.is_empty() {
        entries = fallback_pricing_entries();
    }

    entries.extend(supplemental_pricing_entries());
    entries
}

pub fn load_pricing_rules() -> Vec<PricingRule> {
    load_pricing_entries()
        .into_iter()
        .map(|entry| PricingRule {
            model_name: entry.model_name,
            input_price: entry.input_price,
            cache_input_price: entry.cache_input_price,
            output_price: entry.output_price,
        })
        .collect()
}

pub async fn download_and_update_pricing(
    custom_url: Option<&str>,
) -> Result<PricingUpdateResult, String> {
    let url = custom_url.unwrap_or(LITELLM_REMOTE_URL);
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("無法建立 HTTP 客戶端: {e}"))?;

    let response = client
        .get(url)
        .header("User-Agent", "TokenUsageInsights")
        .send()
        .await
        .map_err(|e| format!("無法下載價格資訊 ({url}): {e}"))?;

    if !response.status().is_success() {
        return Err(format!("下載失敗，HTTP 狀態碼: {}", response.status()));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("讀取回應內容失敗: {e}"))?;

    let parsed: HashMap<String, serde_json::Value> =
        serde_json::from_slice(&bytes).map_err(|e| format!("下載內容非合法 LiteLLM JSON: {e}"))?;

    let models_count = parsed
        .keys()
        .filter(|k| *k != "sample_spec" && *k != "fallback_generalizations")
        .count();
    if models_count < 10 {
        return Err(format!("下載之價格清單模型數量異常不足 ({models_count})"));
    }

    let target_dir = crate::db::get_insights_dir();
    if !target_dir.exists() {
        std::fs::create_dir_all(&target_dir)
            .map_err(|e| format!("無法建立目錄 {}: {e}", target_dir.display()))?;
    }

    let target_file = target_dir.join(LITELLM_PRICING_FILENAME);
    let temp_file = target_dir.join(format!(
        "{LITELLM_PRICING_FILENAME}.tmp.{}",
        std::process::id()
    ));

    std::fs::write(&temp_file, &bytes)
        .map_err(|e| format!("無法寫入暫存檔 {}: {e}", temp_file.display()))?;

    std::fs::rename(&temp_file, &target_file)
        .map_err(|e| format!("無法替換目標檔案 {}: {e}", target_file.display()))?;
    std::fs::write(
        target_dir.join(LITELLM_CACHE_VERSION_FILENAME),
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(|e| format!("無法寫入快取版本標記: {e}"))?;

    Ok(PricingUpdateResult {
        success: true,
        models_count,
        file_path: target_file.to_string_lossy().to_string(),
        message: format!("已成功更新 LiteLLM 模型價格表，共載入 {models_count} 個模型。"),
    })
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

fn strip_reasoning_labels(name: &str) -> String {
    let mut s = name.to_lowercase();
    for pattern in [
        "(extra high)",
        "(high)",
        "(medium)",
        "(low)",
        "(thinking)",
        " · high",
        " · medium",
        " · low",
        " · thinking",
        "-picker",
        ":cloud",
    ] {
        s = s.replace(pattern, "");
    }
    s
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

/// 判斷模型名稱是否為未識別或預設佔位名稱（例如 "Unknown Model"、"Unknown" 等）。
/// 未識別模型無法對應到價格規則，成本估算應直接視為 0.0，不應擲出錯誤或嘗試查詢費率。
pub fn is_unknown_model_name(model: &str) -> bool {
    let trimmed = model.trim();
    trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("unknown")
        || trimmed.eq_ignore_ascii_case("unknown model")
        || trimmed.eq_ignore_ascii_case("unknown_model")
        || trimmed.eq_ignore_ascii_case("<unknown>")
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
            .filter(|name| !name.is_empty() && !is_unknown_model_name(name))
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
        let mut best_distance = usize::MAX;
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

            // 以規則名稱與模型名稱的長度差距作為「貼近程度」：
            // - 模型名稱包含規則名稱（例如 `claude-opus-5-1m · high` 對 `Claude Opus 5`）
            //   時，較長的規則名稱更具體，差距也較小；
            // - 規則名稱包含模型名稱（例如 `opus-5` 對 `Claude Opus 5`）時，
            //   較短的規則才是正確對象，避免被 `Claude Opus 5.5-fast` 之類的
            //   更長名稱搶走。
            let distance = prepared.base.len().abs_diff(model_base.len());
            let has_threshold = prepared.threshold.is_some();
            let is_closer = distance < best_distance;
            let is_same_distance_with_threshold =
                distance == best_distance && has_threshold && !best_has_threshold;
            if best_rule.is_none() || is_closer || is_same_distance_with_threshold {
                best_rule = Some(index);
                best_distance = distance;
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

        let stripped = strip_reasoning_labels(model_name);
        let (s_base, _) = parse_threshold_rule(&stripped);

        // 1. Exact base name match (threshold-aware)
        let matched_index = self
            .find_rule_index(&m_base, prompt_tokens, false)
            // 2. Exact match on stripped reasoning name (e.g. "Gemini 3.8 Flash (High)" -> "Gemini 3.8 Flash")
            .or_else(|| {
                if s_base != m_base {
                    self.find_rule_index(&s_base, prompt_tokens, false)
                } else {
                    None
                }
            })
            // 3. Fallback: contains base name match
            .or_else(|| self.find_rule_index(&m_base, prompt_tokens, true))
            .or_else(|| {
                if s_base != m_base {
                    self.find_rule_index(&s_base, prompt_tokens, true)
                } else {
                    None
                }
            });

        if let Some(index) = matched_index {
            let r = &self.rules[index];
            let is_fast_mode = model_name.to_lowercase().contains("fast");
            let rule_is_fast = r.model_name.to_lowercase().contains("fast");
            let fast_multiplier = if is_fast_mode && !rule_is_fast {
                2.0
            } else {
                1.0
            };

            let input_cost = (input as f64 / 1_000_000.0) * r.input_price * fast_multiplier;
            let cache_cost =
                (cache_read as f64 / 1_000_000.0) * r.cache_input_price * fast_multiplier;
            let cache_write_5m_cost = (priced_cache_write_5m as f64 / 1_000_000.0)
                * r.input_price
                * 1.25
                * fast_multiplier;
            let cache_write_1h_cost = (priced_cache_write_1h as f64 / 1_000_000.0)
                * r.input_price
                * 2.0
                * fast_multiplier;
            let output_cost = (output as f64 / 1_000_000.0) * r.output_price * fast_multiplier;
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
        .filter(|name| !name.is_empty() && !is_unknown_model_name(name))
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
    fn token_usage_with_unknown_model_reports_missing_metadata() {
        for unknown in [
            "Unknown Model",
            "unknown model",
            "Unknown",
            "unknown",
            " unknown_model ",
            "<unknown>",
        ] {
            let error = calculate_usage_cost(&[], Some(unknown), 10, 2, 3, 0, 0).unwrap_err();
            assert_eq!(error, "缺少模型名稱，無法估算成本");
        }
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

    /// 針對同一組模型標籤逐一驗證：各 1M 輸入、輸出與快取讀取 Token 的總成本。
    fn assert_packaged_cost_per_million(
        rules: &[PricingRule],
        model_names: &[&str],
        expected: f64,
        label: &str,
    ) {
        for model_name in model_names {
            let cost = calculate_usage_cost(
                rules,
                Some(model_name),
                1_000_000,
                1_000_000,
                1_000_000,
                0,
                0,
            )
            .unwrap_or_else(|err| panic!("{label} pricing missing for {model_name}: {err}"));

            assert!(
                (cost - expected).abs() < 1e-9,
                "unexpected {label} cost for {model_name}: {cost}"
            );
        }
    }

    #[test]
    fn claude_opus_5_5_variants_use_packaged_pricing() {
        let rules = load_pricing_rules();

        // 輸入 $4 + 輸出 $20 + 快取讀取 $0.20（0.05x）
        assert_packaged_cost_per_million(
            &rules,
            &[
                "claude-opus-5-5",
                "Claude Opus 5.5",
                "claude-opus-5-5-1m · high",
                "opus-5-5",
            ],
            24.2,
            "Opus 5.5",
        );
    }

    #[test]
    fn claude_opus_5_5_fast_mode_uses_doubled_pricing() {
        let rules = load_pricing_rules();

        // Fast Mode 為 2x：輸入 $8 + 輸出 $40 + 快取讀取 $0.40
        assert_packaged_cost_per_million(
            &rules,
            &[
                "claude-opus-5-5-fast",
                "Claude Opus 5.5-fast",
                "claude-opus-5-5-fast · high",
            ],
            48.4,
            "Opus 5.5 fast",
        );
    }

    #[test]
    fn claude_sonnet_5_5_variants_use_packaged_pricing() {
        let rules = load_pricing_rules();

        // 輸入 $2 + 輸出 $10 + 快取讀取 $0.10（0.05x）
        assert_packaged_cost_per_million(
            &rules,
            &[
                "claude-sonnet-5-5",
                "Claude Sonnet 5.5",
                "claude-sonnet-5-5-1m · high",
            ],
            12.1,
            "Sonnet 5.5",
        );
    }

    #[test]
    fn claude_sonnet_5_uses_standard_price_after_launch_pricing_became_permanent() {
        let rules = load_pricing_rules();

        // 官方已將 $2 / $10 上市價轉為正式價，快取讀取維持 0.1x 即 $0.20；
        // 同時確認不會誤配到 Sonnet 5.5 的規則。
        assert_packaged_cost_per_million(
            &rules,
            &[
                "claude-sonnet-5",
                "Claude Sonnet 5",
                "claude-sonnet-5 · high",
            ],
            12.2,
            "Sonnet 5",
        );
    }

    #[test]
    fn claude_haiku_5_5_context_tiers_use_packaged_pricing() {
        let rules = load_pricing_rules();

        // 1. Short context (prompt tokens <= 100k): $0.10 / $0.01 / $0.50 per 1M tokens
        for model_name in [
            "claude-haiku-5-5",
            "Claude Haiku 5.5",
            "Claude Haiku 5.5 (<100k)",
            "claude-haiku-5.5",
        ] {
            let cost = calculate_usage_cost(
                &rules,
                Some(model_name),
                40_000,
                10_000,
                30_000,
                20_000,
                10_000,
            )
            .unwrap_or_else(|error| panic!("{model_name} should have a pricing rule: {error}"));

            let expected = (40_000.0 / 1_000_000.0) * 0.10
                + (30_000.0 / 1_000_000.0) * 0.01
                + (20_000.0 / 1_000_000.0) * (0.10 * 1.25)
                + (10_000.0 / 1_000_000.0) * (0.10 * 2.0)
                + (10_000.0 / 1_000_000.0) * 0.50;
            assert!(
                (cost - expected).abs() < 1e-9,
                "unexpected short-context cost for {model_name}: {cost}"
            );
        }

        // 2. Long context (prompt tokens > 100k): $0.50 / $0.05 / $2.50 per 1M tokens
        for model_name in [
            "claude-haiku-5-5",
            "Claude Haiku 5.5",
            "Claude Haiku 5.5 (>100k)",
            "claude-haiku-5.5",
        ] {
            let cost =
                calculate_usage_cost(&rules, Some(model_name), 60_000, 10_000, 30_000, 10_001, 0)
                    .unwrap_or_else(|error| {
                        panic!("{model_name} should have a pricing rule: {error}")
                    });

            let expected = (60_000.0 / 1_000_000.0) * 0.50
                + (30_000.0 / 1_000_000.0) * 0.05
                + (10_001.0 / 1_000_000.0) * (0.50 * 1.25)
                + (10_000.0 / 1_000_000.0) * 2.50;
            assert!(
                (cost - expected).abs() < 1e-9,
                "unexpected long-context cost for {model_name}: {cost}"
            );
        }

        // 3. Regression test for session_id=c3e4ba13-5d3e-43aa-b96b-16d97d7c1224
        // Turn 1: input=2, output=5, cache_read=17,871, cache_write_5m=62,584 -> prompt=80,457 (<=100k)
        let turn_1_cost =
            calculate_usage_cost(&rules, Some("claude-haiku-5-5"), 2, 5, 17_871, 62_584, 0)
                .expect("claude-haiku-5-5 turn 1 cost should calculate successfully");
        let expected_turn_1 = (2.0 / 1_000_000.0) * 0.10
            + (17_871.0 / 1_000_000.0) * 0.01
            + (62_584.0 / 1_000_000.0) * 0.125
            + (5.0 / 1_000_000.0) * 0.50;
        assert!((turn_1_cost - expected_turn_1).abs() < 1e-9);

        // Turn 6: input=2, output=8, cache_read=98,807, cache_write_5m=3,546 -> prompt=102,355 (>100k)
        let turn_6_cost =
            calculate_usage_cost(&rules, Some("claude-haiku-5-5"), 2, 8, 98_807, 3_546, 0)
                .expect("claude-haiku-5-5 turn 6 cost should calculate successfully");
        let expected_turn_6 = (2.0 / 1_000_000.0) * 0.50
            + (98_807.0 / 1_000_000.0) * 0.05
            + (3_546.0 / 1_000_000.0) * 0.625
            + (8.0 / 1_000_000.0) * 2.50;
        assert!((turn_6_cost - expected_turn_6).abs() < 1e-9);
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
    fn deepseek_v4_1_flash_uses_packaged_pricing() {
        let rules = load_pricing_rules();

        for model_name in [
            "deepseek-v4.1-flash",
            "DeepSeek-V4.1-Flash",
            "deepseek-v4.1-flash:cloud",
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

            assert!(
                (cost - 0.87).abs() < 1e-12,
                "unexpected DeepSeek V4.1-Flash cost for {model_name}: {cost}"
            );
        }
    }

    #[test]
    fn mimo_v2_6_pro_uses_packaged_pricing() {
        let rules = load_pricing_rules();

        for model_name in ["mimo-v2.6-pro", "MiMo-V2.6-Pro", "xiaomi/mimo-v2.6-pro"] {
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

            // Input 0.435 + cache read 0.0036 + output 0.87 = 1.3086.
            assert!(
                (cost - 1.3086).abs() < 1e-12,
                "unexpected MiMo-V2.6-Pro cost for {model_name}: {cost}"
            );
        }
    }

    #[test]
    fn mimo_v2_6_flash_uses_packaged_pricing() {
        let rules = load_pricing_rules();

        for model_name in [
            "mimo-v2.6-flash",
            "MiMo-V2.6-Flash",
            "xiaomi/mimo-v2.6-flash",
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

            // Input 0.14 + cache read 0.0028 + output 0.28 = 0.4228.
            assert!(
                (cost - 0.4228).abs() < 1e-12,
                "unexpected MiMo-V2.6-Flash cost for {model_name}: {cost}"
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
    fn gpt_6_sol_and_luna_context_tiers_use_packaged_pricing() {
        let rules = load_pricing_rules();

        for (model_name, input_price, cached_price, output_price) in [
            ("GPT-6 Sol", 2.00, 0.20, 10.00),
            ("GPT-6 Sol (<272k)", 2.00, 0.20, 10.00),
            ("gpt-6-sol", 2.00, 0.20, 10.00),
            ("GPT-6-Sol", 2.00, 0.20, 10.00),
            ("GPT-6 Luna", 0.10, 0.01, 0.50),
            ("GPT-6 Luna (<272k)", 0.10, 0.01, 0.50),
            ("gpt-6-luna", 0.10, 0.01, 0.50),
            ("GPT-6-Luna", 0.10, 0.01, 0.50),
        ] {
            let cost =
                calculate_usage_cost(&rules, Some(model_name), 100_000, 50_000, 50_000, 0, 0)
                    .unwrap_or_else(|error| {
                        panic!("{model_name} should have a pricing rule: {error}")
                    });

            let expected = (100_000.0 / 1_000_000.0) * input_price
                + (50_000.0 / 1_000_000.0) * cached_price
                + (50_000.0 / 1_000_000.0) * output_price;
            assert!(
                (cost - expected).abs() < 1e-9,
                "unexpected short-context cost for {model_name}: {cost}"
            );
        }

        for (model_name, input_price, cached_price, output_price) in [
            ("GPT-6 Sol", 4.00, 0.40, 15.00),
            ("GPT-6 Sol (>272k)", 4.00, 0.40, 15.00),
            ("gpt-6-sol", 4.00, 0.40, 15.00),
            ("GPT-6-Sol", 4.00, 0.40, 15.00),
            ("GPT-6 Luna", 0.20, 0.02, 0.75),
            ("GPT-6 Luna (>272k)", 0.20, 0.02, 0.75),
            ("gpt-6-luna", 0.20, 0.02, 0.75),
            ("GPT-6-Luna", 0.20, 0.02, 0.75),
        ] {
            let cost =
                calculate_usage_cost(&rules, Some(model_name), 300_000, 50_000, 50_000, 0, 0)
                    .unwrap_or_else(|error| {
                        panic!("{model_name} should have a pricing rule: {error}")
                    });

            let expected = (300_000.0 / 1_000_000.0) * input_price
                + (50_000.0 / 1_000_000.0) * cached_price
                + (50_000.0 / 1_000_000.0) * output_price;
            assert!(
                (cost - expected).abs() < 1e-9,
                "unexpected long-context cost for {model_name}: {cost}"
            );
        }
    }

    #[test]
    fn gpt_daybreak_blue_uses_packaged_pricing() {
        let rules = load_pricing_rules();

        // 1M 輸入 + 1M 輸出 + 1M 快取讀取 = 2M prompt tokens (> 272k 長上下文門檻)
        // 8.00 + 0.80 + 30.00 = 38.80
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

            assert!(
                (cost - 38.8).abs() < 1e-9,
                "unexpected cost for {model_name}: {cost}"
            );
        }

        // Regression test for session 01a06d15-a222-7bc2-9c0b-19ec24de2c80 turn 6
        // input: 227, output: 180, cache_read: 42,886 (prompt <= 272k: 4.00 / 0.40 / 20.00)
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
        let expected_turn_6 = (227.0 / 1_000_000.0) * 4.00
            + (42_886.0 / 1_000_000.0) * 0.40
            + (180.0 / 1_000_000.0) * 20.00;
        assert!((turn_6_cost - expected_turn_6).abs() < 1e-9);
    }

    #[test]
    fn gpt_reserve_matches_gpt_5_6_luna_packaged_pricing() {
        let rules = load_pricing_rules();

        // gpt-reserve is an alias of gpt-5.6-luna: input 0.20, cache read 0.02, output 1.20
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
        let expected_turn_951 = (1_021.0 / 1_000_000.0) * 0.20
            + (197_760.0 / 1_000_000.0) * 0.02
            + (1_388.0 / 1_000_000.0) * 1.20;
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

            // LiteLLM github_copilot/mai-code-1.1-flash: input 0.20 + cache read 0.02 + output 1.20 = 1.42
            assert!(
                (cost - 1.42).abs() < 1e-9,
                "unexpected MAI-Code-1.1-Flash cost for {model_name}: {cost}"
            );
        }

        // The 1.1 rule must not shadow the 1.0 rule (or vice versa).
        let flash_1 =
            calculate_usage_cost(&rules, Some("mai-code-1-flash"), 1_000_000, 0, 0, 0, 0).unwrap();
        assert!((flash_1 - 0.75).abs() < 1e-9);
    }

    #[test]
    fn gemini_4_hydrogen_thinking_levels_use_packaged_pricing() {
        let rules = load_pricing_rules();

        for model_name in [
            "Gemini 4 Hydrogen",
            "Gemini 4 Hydrogen (Low)",
            "Gemini 4 Hydrogen (Medium)",
            "Gemini 4 Hydrogen (High)",
            "Gemini 4 Hydrogen (Extra)",
            "gemini-4-hydrogen",
            // Antigravity CLI model ID documented at antigravity.google/docs/models;
            // unmapped IDs fall back to the contains match on the base rule.
            "gemini-4-hydrogen-xhigh",
        ] {
            let input =
                calculate_usage_cost(&rules, Some(model_name), 1_000_000, 0, 0, 0, 0).unwrap();
            let output =
                calculate_usage_cost(&rules, Some(model_name), 0, 1_000_000, 0, 0, 0).unwrap();
            let cached =
                calculate_usage_cost(&rules, Some(model_name), 0, 0, 1_000_000, 0, 0).unwrap();

            // Standard paid tier: input 1.50, cached input 0.075, output 7.50.
            assert!(
                (input - 1.50).abs() < 1e-9,
                "unexpected Gemini 4 Hydrogen input cost for {model_name}: {input}"
            );
            assert!(
                (output - 7.50).abs() < 1e-9,
                "unexpected Gemini 4 Hydrogen output cost for {model_name}: {output}"
            );
            assert!(
                (cached - 0.075).abs() < 1e-9,
                "unexpected Gemini 4 Hydrogen cached input cost for {model_name}: {cached}"
            );
        }
    }

    #[test]
    fn gemini_4_carbon_thinking_levels_use_packaged_pricing() {
        let rules = load_pricing_rules();

        for model_name in [
            "Gemini 4 Carbon",
            "Gemini 4 Carbon (Medium)",
            "Gemini 4 Carbon (High)",
            "Gemini 4 Carbon (Low)",
            "gemini-4-carbon",
        ] {
            let input =
                calculate_usage_cost(&rules, Some(model_name), 1_000_000, 0, 0, 0, 0).unwrap();
            let output =
                calculate_usage_cost(&rules, Some(model_name), 0, 1_000_000, 0, 0, 0).unwrap();
            let cached =
                calculate_usage_cost(&rules, Some(model_name), 0, 0, 1_000_000, 0, 0).unwrap();

            // Standard (post-introductory) rates: input 4.00, cached input 0.20, output 20.00.
            assert!(
                (input - 4.00).abs() < 1e-9,
                "unexpected Gemini 4 Carbon input cost for {model_name}: {input}"
            );
            assert!(
                (output - 20.00).abs() < 1e-9,
                "unexpected Gemini 4 Carbon output cost for {model_name}: {output}"
            );
            assert!(
                (cached - 0.20).abs() < 1e-9,
                "unexpected Gemini 4 Carbon cached input cost for {model_name}: {cached}"
            );
        }
    }

    #[test]
    fn gemini_4_argon_thinking_levels_use_packaged_pricing() {
        let rules = load_pricing_rules();

        for model_name in [
            "Gemini 4 Argon",
            "Gemini 4 Argon (Medium)",
            "Gemini 4 Argon (High)",
            "Gemini 4 Argon (Low)",
            "gemini-4-argon",
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

            // LiteLLM Google AI: input 2.00 + cache read 0.10 + output 10.00 = 12.10
            assert!(
                (cost - 12.10).abs() < 1e-9,
                "unexpected Gemini 4 Argon cost for {model_name}: {cost}"
            );
        }

        // Regression test for session_id=575e5267-0e01-4a17-acb1-3174ccbe5b61 turn_no=1
        let turn_1_cost = calculate_usage_cost(
            &rules,
            Some("Gemini 4 Argon (Medium)"),
            18_389,
            191,
            0,
            0,
            0,
        )
        .expect("Gemini 4 Argon (Medium) turn 1 cost should calculate successfully");
        let expected_turn_1 = (18_389.0 / 1_000_000.0) * 2.00 + (191.0 / 1_000_000.0) * 10.00;
        assert!((turn_1_cost - expected_turn_1).abs() < 1e-9);
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
                (cost - 4.575).abs() < 1e-9,
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
        let expected = (142_420.0 / 1_000_000.0) * 0.75
            + (129_867.0 / 1_000_000.0) * 0.075
            + (76_059.0 / 1_000_000.0) * 3.75;
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
                (cost - 4.575).abs() < 1e-9,
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
                (cost - 4.575).abs() < 1e-9,
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
    fn packaged_grok_47_pricing_uses_context_tiers_and_fast_mode_multiplier() {
        let rules = load_pricing_rules();

        for model_name in [
            "grok-4.7",
            "Grok 4.7 (Low)",
            "Grok 4.7 (Medium)",
            "Grok 4.7 (High)",
            "Grok 4.7 (Extra High)",
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

        for model_name in [
            "grok-4.7-fast",
            "Grok 4.7 Fast",
            "Grok 4.7 Fast (Extra High)",
        ] {
            let short_context =
                calculate_usage_cost(&rules, Some(model_name), 100_000, 1_000_000, 100_000, 0, 0)
                    .unwrap();
            let long_context =
                calculate_usage_cost(&rules, Some(model_name), 201_000, 1_000_000, 0, 0, 0)
                    .unwrap();

            assert!(
                (short_context - 12.5).abs() < 1e-12,
                "unexpected short-context fast cost for {model_name}: {short_context}"
            );
            assert!(
                (long_context - 25.608).abs() < 1e-12,
                "unexpected long-context fast cost for {model_name}: {long_context}"
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
    fn kimi_k3_1_pro_resolves_pricing_from_csv() {
        let rules = load_prepared_pricing_rules();

        for model_name in ["kimi-k3.1-pro", "kimi-k3.1-pro:cloud", "Kimi K3.1 Pro"] {
            for (input, output, cache_read, expected) in [
                (1_000_000, 0, 0, 2.00),
                (0, 0, 1_000_000, 0.20),
                (0, 1_000_000, 0, 10.00),
                (1_000_000, 1_000_000, 1_000_000, 12.20),
            ] {
                let cost = rules
                    .calculate_cost(model_name, input, output, cache_read, 0, 0)
                    .expect("kimi-k3.1-pro should have a pricing rule");
                assert!(
                    (cost - expected).abs() < 1e-12,
                    "unexpected cost for {model_name}: {cost}, expected {expected}"
                );
            }
        }
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
    fn muse_spark_open_resolves_ollama_pricing_from_csv() {
        let rules = load_prepared_pricing_rules();

        for model_name in ["muse-spark-open", "muse-spark-open:cloud"] {
            for (input, output, cache_read, expected) in [
                (1_000_000, 0, 0, 1.25),
                (0, 0, 1_000_000, 0.15),
                (0, 1_000_000, 0, 4.25),
                (1_000_000, 1_000_000, 1_000_000, 5.65),
            ] {
                let cost = rules
                    .calculate_cost(model_name, input, output, cache_read, 0, 0)
                    .expect("Muse Spark Open should have an Ollama pricing rule");
                assert!(
                    (cost - expected).abs() < 1e-12,
                    "unexpected cost for {model_name}: {cost}, expected {expected}"
                );
            }
        }
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

    #[test]
    fn gemini_3_8_flash_calculates_session_turns_with_packaged_and_embedded_fallback() {
        let rules = load_prepared_pricing_rules();

        // Turn 302: session_id=0ebefcf7-dd4a-4335-baf3-16b07361a867 turn_no=302 model=gemini-3.8-flash
        let res1 = rules
            .calculate_usage_cost(Some("gemini-3.8-flash"), 351_731, 33, 0, 0, 0)
            .expect("turn 302 gemini-3.8-flash cost should calculate successfully");
        let expected_turn_302 = (351_731.0 / 1_000_000.0) * 0.75 + (33.0 / 1_000_000.0) * 3.75;
        assert!((res1 - expected_turn_302).abs() < 1e-9);

        // Turn 1: session_id=d6d8e457-39bc-4b97-a658-d6d4c1cab113 turn_no=1 model=Gemini 3.8 Flash (High)
        let res2 = rules
            .calculate_usage_cost(Some("Gemini 3.8 Flash (High)"), 14_528, 261, 0, 0, 0)
            .expect("turn 1 Gemini 3.8 Flash (High) cost should calculate successfully");
        let expected_turn_1 = (14_528.0 / 1_000_000.0) * 0.75 + (261.0 / 1_000_000.0) * 3.75;
        assert!((res2 - expected_turn_1).abs() < 1e-9);

        // 驗證內建 PACKAGED_LITELLM_JSON 可獨立解析並包含 gemini-3.8-flash
        let embedded_entries =
            parse_litellm_json(std::io::Cursor::new(PACKAGED_LITELLM_JSON.as_bytes()))
                .expect("packaged LiteLLM JSON should parse cleanly");
        assert!(
            embedded_entries
                .iter()
                .any(|e| e.model_name == "gemini-3.8-flash"
                    || e.model_name == "gemini/gemini-3.8-flash"),
            "embedded JSON must contain gemini-3.8-flash"
        );

        // 驗證即使完全走 fallback_pricing_entries 也能計算 gemini-3.8-flash
        let fallback_rules = PreparedPricingRules::from_rules(
            fallback_pricing_entries()
                .into_iter()
                .map(|e| PricingRule {
                    model_name: e.model_name,
                    input_price: e.input_price,
                    cache_input_price: e.cache_input_price,
                    output_price: e.output_price,
                })
                .collect(),
        );
        let fb_cost1 = fallback_rules
            .calculate_usage_cost(Some("gemini-3.8-flash"), 1_000_000, 1_000_000, 0, 0, 0)
            .expect("fallback rules must support gemini-3.8-flash");
        assert!((fb_cost1 - 4.50).abs() < 1e-9);

        let fb_cost2 = fallback_rules
            .calculate_usage_cost(
                Some("Gemini 3.8 Flash (High)"),
                1_000_000,
                1_000_000,
                0,
                0,
                0,
            )
            .expect("fallback rules must support Gemini 3.8 Flash (High)");
        assert!((fb_cost2 - 4.50).abs() < 1e-9);
    }

    #[test]
    fn test_parse_litellm_json_extracts_models_and_tiers() {
        let sample_json = r#"{
            "sample_spec": { "input_cost_per_token": 0.0 },
            "custom/test-model": {
                "litellm_provider": "custom",
                "input_cost_per_token": 0.000001,
                "output_cost_per_token": 0.000002,
                "cache_read_input_token_cost": 0.0000001,
                "input_cost_per_token_above_200k_tokens": 0.000003,
                "output_cost_per_token_above_200k_tokens": 0.000006
            }
        }"#;

        let entries = parse_litellm_json(sample_json.as_bytes()).expect("parse should succeed");
        assert!(entries.iter().any(|e| e.model_name == "custom/test-model"));
        assert!(entries
            .iter()
            .any(|e| e.model_name == "custom/test-model (<200k)"));
        assert!(entries
            .iter()
            .any(|e| e.model_name == "custom/test-model (>200k)"));
    }

    #[test]
    fn pricing_cache_only_overrides_packaged_data_for_same_version() {
        let dir = std::env::temp_dir().join(format!("tui-pricing-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 沒有快取檔
        assert!(!pricing_cache_is_current(&dir));

        // 有快取檔但沒有版本標記（舊版本下載）
        std::fs::write(dir.join(LITELLM_PRICING_FILENAME), "{}").unwrap();
        assert!(!pricing_cache_is_current(&dir));

        // 版本不符
        std::fs::write(dir.join(LITELLM_CACHE_VERSION_FILENAME), "0.0.0").unwrap();
        assert!(!pricing_cache_is_current(&dir));

        // 版本相符
        std::fs::write(
            dir.join(LITELLM_CACHE_VERSION_FILENAME),
            format!("{}\n", env!("CARGO_PKG_VERSION")),
        )
        .unwrap();
        assert!(pricing_cache_is_current(&dir));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_download_pricing_rejects_invalid_content() {
        // 驗證給予無效內容的 URL 時會拋出 Err 而非 panic
        let result = download_and_update_pricing(Some("http://127.0.0.1:9/invalid")).await;
        assert!(result.is_err());
    }
}
