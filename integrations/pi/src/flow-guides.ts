import type { PhaseGuide } from "./guidance.js";

function guide(objective: string, human: string, ai: string): PhaseGuide {
  return { objective, human, ai, artifactPrompts: {} };
}

export const flowGuides: Record<string, PhaseGuide> = {
  "corrective-debugging:characterize": guide(
    "Describe the failure precisely before explaining it.",
    "Record observed and expected behavior, reproduction, timing, location, scope, and important non-occurrences.",
    "After the human account exists, clarify observations and missing discriminators without inventing a cause.",
  ),
  "corrective-debugging:model": guide(
    "Make the human's current causal model explicit and falsifiable.",
    "Explain what you currently think is happening, why, and what that explanation predicts.",
    "After the human model exists, challenge assumptions and surface alternatives without selecting the diagnosis.",
  ),
  "corrective-debugging:investigate": guide(
    "Use discriminating evidence and tests to update competing hypotheses.",
    "Choose what to inspect or test, interpret the results, and maintain epistemic discipline.",
    "Inspect authorized code, logs, history, dependencies, and runtime data; propose hypotheses and bounded tests while separating fact from inference.",
  ),
  "corrective-debugging:conclude": guide(
    "Reach a supported diagnosis or explicitly accept that the cause remains unknown.",
    "Judge the evidence, confidence, uncertainty, and risk, then own the diagnostic conclusion.",
    "Test the conclusion against the ledger and expose counterevidence; do not manufacture certainty or decide for the human.",
  ),
  "corrective-debugging:correction": guide(
    "Select a correction that follows from the accepted diagnosis and can be verified.",
    "Choose the approach and own its risk, blast radius, reversibility, and verification strategy.",
    "Compare likely effectiveness, regressions, observability, and alternatives without choosing the correction.",
  ),
  "corrective-debugging:verify": guide(
    "Demonstrate that the original characterized failure is resolved without unacceptable regression.",
    "Repeat the original reproduction and choose adequate regression and runtime checks.",
    "Help analyze tests and observations while keeping code, deployment, and observed behavior separate.",
  ),
  "operational-stabilization:assess": guide(
    "Establish impact, recovery signals, and accountable response mode.",
    "Assess the situation and explicitly set ownership, communication, authority boundaries, and stop conditions.",
    "Organize evidence and clarify scope after the human assessment begins; do not declare response mode or assume command.",
  ),
  "operational-stabilization:respond": guide(
    "Investigate and select the next bounded stabilizing action without waiting for proven root cause.",
    "Maintain the system model and choose an intervention with actor, scope, rollback, expected signals, uncertainty, and stop conditions.",
    "Investigate in parallel, challenge the intervention, and surface risk; never authorize or execute it.",
  ),
  "operational-stabilization:execute-observe": guide(
    "Execute only the authorized intervention and capture its immediate effects.",
    "Ensure an authorized actor performs the action, observes it, and makes stop or rollback decisions.",
    "Interpret observations only. AI has no workflow authority to execute the intervention or write the action record.",
  ),
  "operational-stabilization:verify-recovery": guide(
    "Demonstrate end-to-end recovery or return for another bounded action.",
    "Compare real system and user-visible behavior with the recorded recovery signals and judge residual risk.",
    "Suggest and analyze authorized checks without treating component health as sufficient proof or requiring root cause.",
  ),
  "operational-stabilization:monitor": guide(
    "Observe long enough to detect recurrence or delayed degradation.",
    "Select the observation window and decide whether the system is stable enough to leave response mode.",
    "Inspect authorized signals and surface recurrence or delayed effects; do not shorten the window or declare stability.",
  ),
  "operational-stabilization:outcome": guide(
    "Close response mode with residual risk, causal uncertainty, and follow-up ownership explicit.",
    "Own closure and route debugging, investigation, product, or improvement follow-up as needed.",
    "Summarize and challenge the evidence without treating stabilization as proof of root cause or accepting closure.",
  ),
  "decision:frame": guide(
    "State the exact decision and who is accountable for it.",
    "Define the choice, purpose, owner, stakeholders, scope, deadline, and reversibility.",
    "Expose ambiguity and assumptions after the human framing; do not redefine the decision.",
  ),
  "decision:criteria": guide(
    "Agree on priorities, constraints, and acceptable uncertainty before comparing options.",
    "Set the values, constraints, evidence standard, and uncertainty that will govern the choice.",
    "Test criteria for conflict, invisibility, or omission without steering them toward a preferred answer.",
  ),
  "decision:research": guide(
    "Gather the material evidence needed for this decision.",
    "Set the boundary, judge relevance, and decide whether evidence gaps can be accepted.",
    "Research authorized sources with provenance, contradictions, assumptions, and explicit gaps.",
  ),
  "decision:options": guide(
    "Develop genuinely viable alternatives before comparison.",
    "Offer the first options, then evaluate AI challenges and own the viable set.",
    "Only after the human first pass, challenge assumptions and surface missing alternatives without steering the choice.",
  ),
  "decision:compare": guide(
    "Compare options against the recorded criteria without hiding tradeoffs.",
    "Make and own the comparison, including risk, confidence, sensitivity, and disagreement.",
    "Check consistency and expose hidden tradeoffs after the human comparison; do not replace it with a synthetic answer.",
  ),
  "decision:decide": guide(
    "Make an accountable, explainable, and revisitable choice.",
    "Choose and record rationale, tradeoffs, dissent, unknowns, confidence, and reversibility.",
    "Challenge whether the choice follows from the evidence, but never make or approve it.",
  ),
  "decision:publish": guide(
    "Make the decision durable, understandable, and connected to downstream work.",
    "Record and communicate the decision, affected work, review trigger, and revisit date.",
    "Improve clarity and traceability without communicating externally or closing work without authorization.",
  ),
  "investigation:frame": guide(
    "Define a question that can be answered or responsibly left unresolved.",
    "State the question, motivation, owner, intended use, and what would count as an answer.",
    "Clarify testability after the human framing without turning the question into a decision or implementation task.",
  ),
  "investigation:bound": guide(
    "Constrain the search with an evidence standard and stop conditions.",
    "Set scope, non-goals, time or cost box, risks, and stopping rules.",
    "Expose unbounded searches or unreachable standards without expanding the authorized purpose.",
  ),
  "investigation:gather": guide(
    "Collect relevant evidence with provenance and visible gaps.",
    "Judge source relevance and decide when enough evidence exists to explore explanations.",
    "Gather authorized sources, observations, and measurements; separate facts, inference, contradiction, and uncertainty.",
  ),
  "investigation:explore": guide(
    "Test competing explanations and learn cheaply within the investigation bounds.",
    "Supply the first model or path, select tests, interpret results, and dispose of any prototype.",
    "After the human first pass, challenge it and propose tests or disposable prototypes while keeping production code out of scope.",
  ),
  "investigation:synthesize": guide(
    "Turn the evidence into a traceable account of what is known and unknown.",
    "Synthesize findings, evidence strength, contradictions, limitations, and confidence.",
    "Check completeness and traceability without overstating convergence or writing the human conclusion.",
  ),
  "investigation:conclude": guide(
    "Record an answer or defensible non-answer and route consequential choice explicitly.",
    "Own the conclusion, confidence, unresolved questions, reusable evidence, and any route to Decision.",
    "Summarize and challenge the conclusion without deciding, implementing, or accepting closure.",
  ),
  "internal-improvement:invariants": guide(
    "Protect behavior that must not regress before optimizing anything.",
    "Define testable behavior, compatibility, safety, operability, and other invariants.",
    "Help make invariants observable without trading them away for a metric.",
  ),
  "internal-improvement:baseline": guide(
    "Establish a reproducible current measurement before choosing a change.",
    "Own the method, sample, uncertainty, reproducibility, and accepted baseline.",
    "After the baseline exists, challenge noise, confounders, sampling, and observer effects.",
  ),
  "internal-improvement:target": guide(
    "Set a measurable improvement target with guardrails and stop conditions.",
    "Choose the desired change, threshold, scope, guardrails, and acceptable tradeoffs.",
    "Test for gaming, shifted cost, and proxy failure without choosing the target.",
  ),
  "internal-improvement:options": guide(
    "Develop alternatives that can improve the target while preserving invariants.",
    "Provide the first approach, then evaluate alternatives and own the final option set.",
    "After the human first pass, challenge assumptions, alternatives, maintenance cost, and measurement risk.",
  ),
  "internal-improvement:decision": guide(
    "Choose an improvement approach that follows from the baseline, target, and invariants.",
    "Own the selected option, rationale, tradeoffs, risks, confidence, and reversibility.",
    "Check the relationship to the evidence and guardrails without choosing or approving the approach.",
  ),
  "internal-improvement:verify": guide(
    "Show that invariants hold and the measured improvement is real enough to adopt.",
    "Repeat comparable measurements, assess noise and regressions, and judge the result.",
    "Analyze before-and-after evidence without cherry-picking a favorable number or declaring success.",
  ),
  "internal-improvement:outcome": guide(
    "Decide whether to adopt, roll back, accept a tradeoff, or continue learning.",
    "Own the outcome, measurement uncertainty, follow-up, and learning.",
    "Challenge unsupported success claims and organize evidence without accepting closure.",
  ),
};
