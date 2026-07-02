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
    let file_path = PathBuf::from("pricing.csv");
    if let Ok(file) = File::open(&file_path) {
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        if let Some(Ok(_header)) = lines.next() {
            for line in lines.flatten() {
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

fn parse_threshold_rule(name: &str) -> (String, Option<bool>) {
    let lower = name.to_lowercase();
    let is_greater = if lower.contains(">272k") || lower.contains("> 272k") {
        Some(true)
    } else if lower.contains("<272k") || lower.contains("< 272k") {
        Some(false)
    } else {
        None
    };

    let base = lower
        .replace("<272k", "")
        .replace(">272k", "")
        .replace("(<272k)", "")
        .replace("(>272k)", "")
        .replace("(<272k context length)", "")
        .replace("(>272k context length)", "");

    let normalized = base.chars().filter(|c| c.is_alphanumeric()).collect();

    (normalized, is_greater)
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
) -> f64 {
    let (m_base, _) = parse_threshold_rule(model_name);
    if m_base.is_empty() {
        return 0.0;
    }

    let total_context = input + cache_read + output;
    let is_long_context = total_context > 272_000;

    // 1. First attempt: exact base name match
    let mut rule = rules.iter().find(|r| {
        let (r_base, r_thresh) = parse_threshold_rule(&r.model_name);
        if r_base.is_empty() {
            return false;
        }
        if r_base != m_base {
            return false;
        }
        match r_thresh {
            Some(greater) => {
                if greater {
                    is_long_context
                } else {
                    !is_long_context
                }
            }
            None => true,
        }
    });

    // 2. Fallback: contains base name match
    if rule.is_none() {
        rule = rules.iter().find(|r| {
            let (r_base, r_thresh) = parse_threshold_rule(&r.model_name);
            if r_base.is_empty() {
                return false;
            }
            let base_matches = m_base.contains(&r_base) || r_base.contains(&m_base);
            if !base_matches {
                return false;
            }
            match r_thresh {
                Some(greater) => {
                    if greater {
                        is_long_context
                    } else {
                        !is_long_context
                    }
                }
                None => true,
            }
        });
    }

    if let Some(r) = rule {
        let input_cost = (input as f64 / 1_000_000.0) * r.input_price;
        let cache_cost = (cache_read as f64 / 1_000_000.0) * r.cache_input_price;
        let output_cost = (output as f64 / 1_000_000.0) * r.output_price;
        input_cost + cache_cost + output_cost
    } else {
        0.0 // 未知模型暫估為 0 元
    }
}
