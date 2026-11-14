# Why I Chose [CHOICE] for [PROJECT/SYSTEM]

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** [Proposed | Accepted | Superseded | Deprecated]  
**Date:** [YYYY-MM-DD]  
**Project:** [Project Name]  
**Owner:** [Your Name / Role]  
**Supersedes:** [Prior decision, or None]  
**Scope:** [What this decision covers and what it does not cover]

---

## Summary

This document records why I chose [CHOICE] for [PROJECT/SYSTEM]. It is both an official decision record and a devlog entry: the decision is [final/proposed] for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: [One to three sentences stating the core reason and the main tradeoff.]

---

## 1. Context

### 1.1 The project

[Describe the project and what makes it demanding. List the specific technical characteristics that constrain the choice, e.g. performance, memory, concurrency, tooling, long-term maintenance.]

### 1.2 The constraints

My choice is constrained by the following:

- [Constraint 1]
- [Constraint 2]
- [Constraint 3]
- [Constraint 4]

---

## 2. Decision

**[CHOICE] is the [primary/selected] [tool/language/architecture/approach] for [scope].**

This includes [specific components or areas covered]. [Other tools/languages] remain acceptable for [out-of-scope areas]. I am not selecting [rejected alternatives]. I accept this decision for the current scope and will revisit it only if the assumptions in this document stop holding.

---

## 3. Why I Chose [CHOICE]

### 3.1 [Technical reason 1]
[Explain the technical rationale, tied to the project's demands.]

### 3.2 [Technical reason 2]
[Explain the technical rationale, tied to the project's demands.]

### 3.3 [Safety / typing / tooling reason]
[Explain how this choice improves code health, tooling, or compile-time safety.]

### 3.4 [Discipline or workflow reason]
[Explain how this choice enforces good practice, even if it causes friction.]

### 3.5 [Personal preference]
[Explain any personal preference factors honestly, and note that they are legitimate but secondary to technical requirements.]

### 3.6 [Avoidance of rejected options]
[Explain why other categories of tools/languages were rejected.]

### 3.7 Why not [Alternative A] or [Alternative B]?
- **[Alternative A]:** [Reason for rejection or limited use.]
- **[Alternative B]:** [Reason for rejection or limited use.]

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **[CHOICE]** | [Strengths.] | **Selected** |
| **[Alternative A]** | [Strengths and weaknesses.] | [Retained for X / Not selected] |
| **[Alternative B]** | [Strengths and weaknesses.] | [Not selected] |
| **[Alternative C]** | [Strengths and weaknesses.] | [Not selected] |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the record stays honest.

| Counterargument | Disposition |
|---|---|
| **[Counterargument 1]** | [Accepted / Accepted with qualification / Rejected with reasoning.] |
| **[Counterargument 2]** | [Accepted / Accepted with qualification / Rejected with reasoning.] |
| **[Counterargument 3]** | [Accepted / Accepted with qualification / Rejected with reasoning.] |
| **[Counterargument 4]** | [Accepted / Accepted with qualification / Rejected with reasoning.] |

---

## 6. Consequences

### 6.1 Positive

- [Positive consequence 1]
- [Positive consequence 2]
- [Positive consequence 3]

### 6.2 Negative

- [Negative consequence 1]
- [Negative consequence 2]
- [Negative consequence 3]

### 6.3 Neutral

- [Neutral consequence 1]
- [Neutral consequence 2]

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| [Risk 1] | [Mitigation 1] |
| [Risk 2] | [Mitigation 2] |
| [Risk 3] | [Mitigation 3] |
| [Risk 4] | [Mitigation 4] |

---

## 8. Conclusion

[CHOICE] is my deliberate, [preference-driven / requirement-driven], [tradeoff-aware] choice for [PROJECT/SYSTEM]. I am selecting it because [core reasons]. I acknowledge that [main weakness or counterargument] and manage it through the risks and mitigations above. I therefore accept this decision as [scope statement], with [exceptions or retained alternatives] and with its limits recorded above.

---

*This entry is part of my [PROJECT] decision log. Future entries should reference this document when they depend on this decision or revise its assumptions.*

---

## How to Use This Template

- **Keep the metadata header.** It makes each document traceable and consistent.
- **Write in first person.** It is a devlog and a decision record, so “I chose” is appropriate.
- **Do not skip Section 5.** Recording counterarguments honestly is what makes the document credible.
- **Use the same section numbering across documents.** It makes the decision log searchable and easy to cross-reference.