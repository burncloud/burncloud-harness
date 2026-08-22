use crate::analysis::AnalysisReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalPriority {
    High,
    Medium,
    Low,
}

impl ProposalPriority {
    fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::High => 3,
            Self::Medium => 2,
            Self::Low => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImprovementProposal {
    pub priority: ProposalPriority,
    pub layer: &'static str,
    pub context_kind: &'static str,
    pub context: String,
    pub failure_class: String,
    pub count: usize,
    pub suggested_change: &'static str,
}

impl ImprovementProposal {
    pub fn render(&self, index: usize) -> String {
        format!(
            "{}. [{}] {}\n   evidence: {} / {} -> {}x {}\n   proposal: {}",
            index,
            self.priority.as_str(),
            self.layer,
            self.context_kind,
            self.context,
            self.count,
            self.failure_class,
            self.suggested_change
        )
    }
}

pub fn build(report: &AnalysisReport, min_count: usize) -> Vec<ImprovementProposal> {
    let threshold = min_count.max(1);
    let mut proposals = Vec::new();

    collect_hotspots(&mut proposals, "area", &report.failure_by_area, threshold);
    collect_hotspots(
        &mut proposals,
        "changed-domain",
        &report.failure_by_domain,
        threshold,
    );

    proposals.sort_by(|left, right| {
        right
            .priority
            .rank()
            .cmp(&left.priority.rank())
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.context_kind.cmp(right.context_kind))
            .then_with(|| left.context.cmp(&right.context))
            .then_with(|| left.failure_class.cmp(&right.failure_class))
    });
    proposals
}

pub fn render(proposals: &[ImprovementProposal], min_count: usize) -> String {
    let mut output = String::new();
    output.push_str("BurnCloud Harness Improvement Proposals\n");
    output.push_str(&format!(
        "evidence_threshold={} policy_mutation=disabled\n",
        min_count.max(1)
    ));

    if proposals.is_empty() {
        output.push_str("\nNo evidence-backed proposal meets the current threshold. Keep collecting trajectories.\n");
        return output;
    }

    output.push_str("\nThese are read-only engineering proposals. They do not change prompts, permissions, invariants, tests, or protected policy.\n\n");
    for (index, proposal) in proposals.iter().enumerate() {
        output.push_str(&proposal.render(index + 1));
        output.push('\n');
    }
    output
}

fn collect_hotspots(
    proposals: &mut Vec<ImprovementProposal>,
    context_kind: &'static str,
    hotspots: &std::collections::BTreeMap<String, usize>,
    threshold: usize,
) {
    for (key, count) in hotspots {
        if *count < threshold {
            continue;
        }
        let Some((context, failure_class)) = key.rsplit_once(" / ") else {
            continue;
        };
        let (priority, layer, suggested_change) = recommendation(failure_class, *count);
        proposals.push(ImprovementProposal {
            priority,
            layer,
            context_kind,
            context: context.to_owned(),
            failure_class: failure_class.to_owned(),
            count: *count,
            suggested_change,
        });
    }
}

fn recommendation(
    failure_class: &str,
    count: usize,
) -> (ProposalPriority, &'static str, &'static str) {
    let base_priority = match failure_class {
        "scope_violation" | "git_history" | "risk_block" => ProposalPriority::High,
        "verification" | "invariant_expansion" | "risk_review" | "max_loops" => {
            ProposalPriority::Medium
        }
        "agent_command" | "no_change" => ProposalPriority::Low,
        _ => ProposalPriority::Low,
    };
    let priority = if count >= 10 && base_priority == ProposalPriority::Medium {
        ProposalPriority::High
    } else if count >= 10 && base_priority == ProposalPriority::Low {
        ProposalPriority::Medium
    } else {
        base_priority
    };

    let (layer, change) = match failure_class {
        "scope_violation" => (
            "Scope / ownership",
            "Tighten the task template or ownership boundary for this hotspot. Make the expected allowlist explicit earlier; do not widen write permissions automatically.",
        ),
        "git_history" => (
            "Capability boundary",
            "Keep the hard Git-history block and reduce the worker's opportunity to invoke commit/history-changing commands in this hotspot.",
        ),
        "risk_block" => (
            "Risk prevention",
            "Keep the deterministic block. Improve pre-change guidance or tools so the worker is less likely to attempt this protected change; never weaken the blocker merely because it repeats.",
        ),
        "verification" => (
            "Verification",
            "Move the most relevant BurnCloud check earlier for this hotspot or add a cheaper targeted preflight, while keeping the final invariant gate authoritative.",
        ),
        "invariant_expansion" => (
            "Task routing / invariants",
            "Inspect which invariant IDs repeatedly expand here. If the ownership relationship is stable in BurnCloud source/docs, promote it into pre-change invariant selection for this hotspot.",
        ),
        "risk_review" => (
            "Risk classification",
            "Inspect reviewed patches for a stable common pattern. Promote only high-confidence cases to a deterministic rule; otherwise keep the explicit review step.",
        ),
        "max_loops" => (
            "Loop / diagnostics",
            "Improve early failure evidence or split the task contract into a smaller BurnCloud change boundary so the worker converges before exhausting the retry budget.",
        ),
        "agent_command" => (
            "Agent runtime",
            "Inspect invocation/tool failures in this hotspot and improve deterministic runtime feedback before changing coding policy.",
        ),
        "no_change" => (
            "Task contract",
            "Strengthen completion evidence for this hotspot so the worker can distinguish a required code change from a justified no-op before consuming another loop.",
        ),
        _ => (
            "Observation",
            "Collect more trajectory evidence and inspect representative runs before changing Harness behavior.",
        ),
    };

    (priority, layer, change)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn builds_proposals_only_above_threshold() {
        let report = AnalysisReport {
            failure_by_area: BTreeMap::from([
                ("router / verification".into(), 7),
                ("auth / risk_review".into(), 2),
            ]),
            failure_by_domain: BTreeMap::from([("router / invariant_expansion".into(), 4)]),
            ..AnalysisReport::default()
        };

        let proposals = build(&report, 3);
        assert_eq!(proposals.len(), 2);
        assert!(proposals
            .iter()
            .any(|proposal| proposal.failure_class == "verification"));
        assert!(proposals
            .iter()
            .any(|proposal| proposal.failure_class == "invariant_expansion"));
    }

    #[test]
    fn protected_failures_receive_high_priority() {
        let report = AnalysisReport {
            failure_by_domain: BTreeMap::from([("server / risk_block".into(), 3)]),
            ..AnalysisReport::default()
        };
        let proposals = build(&report, 3);
        assert_eq!(proposals[0].priority, ProposalPriority::High);
        assert_eq!(proposals[0].layer, "Risk prevention");
    }

    #[test]
    fn repeated_medium_failure_escalates_priority() {
        let report = AnalysisReport {
            failure_by_area: BTreeMap::from([("router / verification".into(), 10)]),
            ..AnalysisReport::default()
        };
        let proposals = build(&report, 3);
        assert_eq!(proposals[0].priority, ProposalPriority::High);
    }
}
