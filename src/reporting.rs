use std::collections::HashMap;

use crate::{
    db::UsageEntry,
    handlers::{
        summarize_session_usage, AgentBreakdown, DaySummary, MonthlyModelSummary,
        MonthlyProjectSummary, UsageAggregation,
    },
    pricing::PreparedPricingRules,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SessionIdentity {
    pub assistant_type: String,
    pub source_kind: String,
    pub session_id: String,
    pub source_dir_key: Option<String>,
}

impl SessionIdentity {
    pub(crate) fn from_entry(assistant_type: &str, entry: &UsageEntry) -> Self {
        Self {
            assistant_type: assistant_type.to_string(),
            source_kind: entry
                .source_kind
                .clone()
                .unwrap_or_else(|| "legacy".to_string()),
            session_id: entry.session_id.clone(),
            source_dir_key: entry.source_dir_key.clone(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct SessionGroup {
    pub entries: Vec<UsageEntry>,
}

pub(crate) type SessionMap = HashMap<SessionIdentity, SessionGroup>;

pub(crate) fn group_sessions<'a>(
    entries: impl IntoIterator<Item = (&'a UsageEntry, &'a str)>,
) -> SessionMap {
    let mut sessions = HashMap::new();
    for (entry, assistant_type) in entries {
        sessions
            .entry(SessionIdentity::from_entry(assistant_type, entry))
            .or_insert_with(|| SessionGroup {
                entries: Vec::new(),
            })
            .entries
            .push(entry.clone());
    }
    sessions
}

pub(crate) fn cursor_session_mode(
    assistant_type: &str,
    entries: &[UsageEntry],
) -> Option<&'static str> {
    if assistant_type != "cursor" {
        return None;
    }

    entries
        .iter()
        .max_by(|left, right| {
            left.turn_no
                .cmp(&right.turn_no)
                .then_with(|| left.timestamp.cmp(&right.timestamp))
        })
        .and_then(|entry| match entry.source_kind.as_deref() {
            Some("cursor-agent") => Some("agent"),
            Some("cursor-ide") => Some("ide"),
            _ => None,
        })
}

pub(crate) fn summarize_models_by_mode(
    sessions: &SessionMap,
    pricing_rules: &PreparedPricingRules,
) -> Vec<MonthlyModelSummary> {
    type ModelStats = (usize, u64, u64, u64, u64, f64);

    let mut stats: HashMap<(String, Option<String>), ModelStats> = HashMap::new();
    for (identity, group) in sessions {
        let session_usage = summarize_session_usage(pricing_rules, &group.entries);
        let mode =
            cursor_session_mode(&identity.assistant_type, &group.entries).map(str::to_string);
        for model_usage in session_usage.models {
            let model_stat = stats
                .entry((model_usage.model, mode.clone()))
                .or_insert((0, 0, 0, 0, 0, 0.0));
            model_stat.0 += 1;
            model_stat.1 += model_usage.usage.total_tokens;
            model_stat.2 += model_usage.usage.input_tokens;
            model_stat.3 += model_usage.usage.output_tokens;
            model_stat.4 += model_usage.usage.cache_read_tokens;
            model_stat.5 += model_usage.usage.cost_usd;
        }
    }

    let mut summaries = stats
        .into_iter()
        .map(
            |(
                (model, mode),
                (
                    sessions_count,
                    total_tokens,
                    total_input_tokens,
                    total_output_tokens,
                    total_cache_read_tokens,
                    cost_usd,
                ),
            )| MonthlyModelSummary {
                model,
                mode,
                sessions_count,
                total_tokens,
                total_input_tokens,
                total_output_tokens,
                total_cache_read_tokens,
                cost_usd,
            },
        )
        .collect::<Vec<_>>();
    summaries.sort_by_key(|item| std::cmp::Reverse(item.total_tokens));
    summaries
}

#[derive(Debug)]
pub(crate) struct PeriodBreakdown {
    pub label: String,
    pub usage: UsageAggregation,
    pub sessions_count: usize,
}

pub(crate) struct PeriodReport {
    pub summary: DaySummary,
    pub breakdown: Vec<PeriodBreakdown>,
    pub projects: Vec<MonthlyProjectSummary>,
    pub models: Vec<MonthlyModelSummary>,
    pub agent_breakdown: HashMap<String, AgentBreakdown>,
}

fn add_usage(summary: &mut DaySummary, usage: &UsageAggregation) {
    summary.total_tokens += usage.total_tokens;
    summary.total_input_tokens += usage.input_tokens;
    summary.total_output_tokens += usage.output_tokens;
    summary.total_cache_read_tokens += usage.cache_read_tokens;
    summary.total_cache_write_tokens += usage.cache_write_tokens;
    summary.total_reasoning_tokens += usage.reasoning_tokens;
    summary.total_cost_usd += usage.cost_usd;
}

fn summarize_groups(
    sessions: &SessionMap,
    pricing_rules: &PreparedPricingRules,
) -> UsageAggregation {
    let mut usage = UsageAggregation::default();
    for group in sessions.values() {
        let session = summarize_session_usage(pricing_rules, &group.entries);
        usage.total_tokens += session.usage.total_tokens;
        usage.input_tokens += session.usage.input_tokens;
        usage.output_tokens += session.usage.output_tokens;
        usage.cache_read_tokens += session.usage.cache_read_tokens;
        usage.cache_write_tokens += session.usage.cache_write_tokens;
        usage.reasoning_tokens += session.usage.reasoning_tokens;
        usage.cost_usd += session.usage.cost_usd;
    }
    usage
}

pub(crate) fn build_period_report(
    entries: &[(UsageEntry, String, String)],
    bucket_label: impl Fn(&str) -> String,
    pricing_rules: &PreparedPricingRules,
) -> PeriodReport {
    let sessions = group_sessions(
        entries
            .iter()
            .map(|(entry, assistant_type, _)| (entry, assistant_type.as_str())),
    );
    let mut buckets: HashMap<String, Vec<(UsageEntry, String)>> = HashMap::new();
    for (entry, assistant_type, entry_date) in entries {
        buckets
            .entry(bucket_label(entry_date))
            .or_default()
            .push((entry.clone(), assistant_type.clone()));
    }

    let mut labels = buckets.keys().cloned().collect::<Vec<_>>();
    labels.sort();
    let mut summary = DaySummary {
        total_sessions: sessions.len(),
        ..Default::default()
    };
    let mut breakdown = Vec::with_capacity(labels.len());
    for label in labels {
        let bucket_sessions = group_sessions(
            buckets[&label]
                .iter()
                .map(|(entry, assistant_type)| (entry, assistant_type.as_str())),
        );
        let usage = summarize_groups(&bucket_sessions, pricing_rules);
        add_usage(&mut summary, &usage);
        breakdown.push(PeriodBreakdown {
            label,
            usage,
            sessions_count: bucket_sessions.len(),
        });
    }

    let mut project_stats: HashMap<String, (usize, u64, f64)> = HashMap::new();
    let mut agent_breakdown: HashMap<String, AgentBreakdown> = HashMap::new();
    for (identity, group) in &sessions {
        let Some(latest) = group.entries.iter().max_by(|left, right| {
            left.turn_no
                .cmp(&right.turn_no)
                .then_with(|| left.timestamp.cmp(&right.timestamp))
        }) else {
            continue;
        };
        let session = summarize_session_usage(pricing_rules, &group.entries);
        let cwd = latest
            .cwd
            .clone()
            .unwrap_or_else(|| "Unknown CWD".to_string());
        let project = project_stats.entry(cwd).or_insert((0, 0, 0.0));
        project.0 += 1;
        project.1 += session.usage.total_tokens;
        project.2 += session.usage.cost_usd;

        let agent = agent_breakdown
            .entry(identity.assistant_type.clone())
            .or_default();
        agent.total_tokens += session.usage.total_tokens;
        agent.total_input_tokens += session.usage.input_tokens;
        agent.total_output_tokens += session.usage.output_tokens;
        agent.total_cache_read_tokens += session.usage.cache_read_tokens;
        agent.total_reasoning_tokens += session.usage.reasoning_tokens;
        agent.total_cost_usd += session.usage.cost_usd;
        agent.total_sessions += 1;
    }

    let mut projects = project_stats
        .into_iter()
        .map(
            |(cwd, (sessions_count, total_tokens, cost_usd))| MonthlyProjectSummary {
                cwd,
                sessions_count,
                total_tokens,
                cost_usd,
            },
        )
        .collect::<Vec<_>>();
    projects.sort_by_key(|item| std::cmp::Reverse(item.total_tokens));
    let models = summarize_models_by_mode(&sessions, pricing_rules);

    PeriodReport {
        summary,
        breakdown,
        projects,
        models,
        agent_breakdown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::TokenStats;

    fn usage_entry(source_dir_key: &str, model: &str, total_tokens: u64) -> UsageEntry {
        let tokens = TokenStats {
            input: total_tokens,
            output: 0,
            cache_read: Some(0),
            cache_write: Some(0),
            cache_write_5m: None,
            cache_write_1h: None,
            reasoning: None,
            total: total_tokens,
        };
        UsageEntry {
            timestamp: "2026-07-10T10:00:00Z".to_string(),
            session_id: "shared".to_string(),
            session_name: Some("Shared".to_string()),
            transcript_path: None,
            cwd: Some(format!("/workspace/{source_dir_key}")),
            version: None,
            turn_no: 1,
            model: Some(model.to_string()),
            model_id: Some(model.to_string()),
            tokens: Some(tokens.clone()),
            delta_tokens: Some(tokens),
            context: None,
            cost: None,
            source_kind: Some("copilot-app".to_string()),
            source_dir_key: Some(source_dir_key.to_string()),
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        }
    }

    #[test]
    fn period_report_keeps_same_id_source_directories_separate() {
        let entries = vec![
            (
                usage_entry("aa", "gpt-5", 100),
                "copilot".to_string(),
                "2026-07-10".to_string(),
            ),
            (
                usage_entry("aa", "claude-sonnet-4", 200),
                "copilot".to_string(),
                "2026-07-10".to_string(),
            ),
            (
                usage_entry("bb", "gpt-5", 300),
                "copilot".to_string(),
                "2026-07-10".to_string(),
            ),
        ];

        let report = build_period_report(
            &entries,
            str::to_string,
            &PreparedPricingRules::from_rules(Vec::new()),
        );

        assert_eq!(report.summary.total_sessions, 2);
        assert_eq!(report.breakdown[0].sessions_count, 2);
        let gpt_summary = report
            .models
            .iter()
            .find(|summary| summary.model == "gpt-5")
            .unwrap();
        assert_eq!(gpt_summary.sessions_count, 2);
        assert_eq!(gpt_summary.total_tokens, 400);
    }
}
