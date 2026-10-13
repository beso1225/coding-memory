# Coding Memory

Coding Memory preserves reusable knowledge from coding-agent work so that later
runs can retrieve it with its evidence and applicability intact.

## Language

**Project**:
A codebase whose memories share an identity and applicability boundary. Branches
and worktrees of the same repository belong to the same project.
_Avoid_: Working directory, worktree

**Run**:
A runtime execution associated with a turn and its final outcome. Additional
instructions delivered through steering may belong to the same run.
_Avoid_: User message, session, tool step

**Evidence Bundle**:
A captured, bounded account of a run, with references to the messages and tool
results that support knowledge derived from it. Its coverage records what was
available and what was omitted.
_Avoid_: Full trace, complete execution record

**Evidence Reference**:
A locator for a specific part of an evidence bundle that supports a claim.
_Avoid_: Citation without a locator

**Candidate**:
A proposed reusable piece of knowledge that has not been accepted into long-term
memory. A candidate can be awaiting consolidation, held, rejected, or incorporated.
_Avoid_: Memory, note

**Memory Note**:
An accepted unit of reusable knowledge with an identity, applicability, evidence,
and verification state. One independent piece of knowledge forms one memory note.
_Avoid_: Conversation summary, progress log

**Scope**:
The boundary within which a memory is eligible for retrieval: a particular project
or explicitly general knowledge.
_Avoid_: Directory, topic

**Applicability Condition**:
A constraint on when a claim can be used, such as a software version, commit,
platform, or observed environment.
_Avoid_: Tag, confidence

**Extraction**:
The identification of reusable knowledge candidates from a run's evidence.
_Avoid_: Summarization

**Consolidation**:
The reconciliation of candidates with each other and with existing memories to
produce proposed additions, updates, supersessions, and relationships.
_Avoid_: Extraction, deduplication alone

**Change Set**:
A collection of proposed memory changes with their supporting evidence and the
existing memory revisions they assume.
_Avoid_: Generated files, unreviewed patch

**Review**:
An assessment of whether a proposed memory change is supported by its supplied
evidence and preserves the relevant conditions and relationships.
_Avoid_: Verification, proof

**Verification State**:
The kind and extent of support for a claim, including direct observation, an
explicit user statement, or an unverified inference. Review acceptance does not
change the kind of evidence that was obtained.
_Avoid_: LLM confidence, reviewer approval

**Hold**:
A state in which a candidate or proposed change is retained without publication
because evidence, conflict resolution, or processing is incomplete.
_Avoid_: Rejection, deletion

**Supersession**:
The replacement of a memory by a successor when its premise or conclusion has
changed. The predecessor remains available for historical inspection.
_Avoid_: Minor update, deletion

**Related Memory**:
A memory whose relationship to another memory is supported by their content and
applicability. A shared keyword alone is not sufficient to establish the relation.
_Avoid_: Keyword match, proven dependency
