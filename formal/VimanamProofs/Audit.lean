/-
# Axiom audit

`#print axioms` for every theorem under `VimanamProofs/`. CI elaborates this
file and requires each answer to name only Lean's standard axioms
(`propext`, `Quot.sound`, `Classical.choice`), and one answer per line here.
CI also fails when a `theorem` under `VimanamProofs/` has no line here.

-/
import VimanamProofs

#print axioms Vimanam.Severity.mem_allKinds
#print axioms Vimanam.Severity.mem_allLocs
#print axioms Vimanam.Severity.forall_of_check
#print axioms Vimanam.Severity.locateGo_append
#print axioms Vimanam.Severity.underNotGo_append
#print axioms Vimanam.Severity.walk_append
#print axioms Vimanam.Severity.locate_nil
#print axioms Vimanam.Severity.property_named_like_a_keyword
#print axioms Vimanam.Severity.underNot_of_not
#print axioms Vimanam.Severity.not_is_review
#print axioms Vimanam.Severity.requestRule_narrows
#print axioms Vimanam.Severity.requestRule_widens
#print axioms Vimanam.Severity.responseRule_widens
#print axioms Vimanam.Severity.responseRule_narrows
#print axioms Vimanam.Severity.effect_of_bodyDropped
#print axioms Vimanam.Severity.request_narrowing_breaking
#print axioms Vimanam.Severity.request_widening_not_breaking
#print axioms Vimanam.Severity.response_widening_not_nonBreaking
#print axioms Vimanam.Severity.response_narrowing_not_breaking
#print axioms Vimanam.Severity.effect_swap
#print axioms Vimanam.Severity.requestRule_swap
#print axioms Vimanam.Severity.responseRule_swap
#print axioms Vimanam.Severity.request_swap
#print axioms Vimanam.Severity.response_swap
#print axioms Vimanam.Severity.enum_keyword_added
#print axioms Vimanam.Severity.enum_keyword_removed
#print axioms Vimanam.Severity.requestRule_review
#print axioms Vimanam.Severity.responseRule_review
#print axioms Vimanam.Severity.request_review_only_when_undirected
#print axioms Vimanam.Severity.response_review_only_when_undirected
#print axioms Vimanam.Severity.parameter_schema_is_request
#print axioms Vimanam.Severity.parameter_required_like_schema_required
#print axioms Vimanam.Severity.removal_breaking_addition_not
#print axioms Vimanam.Severity.operation_id
#print axioms Vimanam.Severity.requestRule_noop_breaking
#print axioms Vimanam.Severity.responseRule_noop_breaking
#print axioms Vimanam.Severity.request_noop_breaking
#print axioms Vimanam.Severity.request_noop_breaking_witness
#print axioms Vimanam.Severity.response_noop_breaking
#print axioms Vimanam.Severity.response_noop_breaking_witness

#print axioms Vimanam.EnumReport.locate_member
#print axioms Vimanam.EnumReport.underNot_member
#print axioms Vimanam.EnumReport.locate_enum
#print axioms Vimanam.EnumReport.member_added
#print axioms Vimanam.EnumReport.member_removed
#print axioms Vimanam.EnumReport.underNot_enum
#print axioms Vimanam.EnumReport.report_ptr
#print axioms Vimanam.EnumReport.response_widening_flagged
#print axioms Vimanam.EnumReport.request_narrowing_breaking
#print axioms Vimanam.EnumReport.request_widening_not_breaking

#print axioms Vimanam.History.outside
#print axioms Vimanam.History.pre175_response_narrowing_breaking
#print axioms Vimanam.History.round1_enum_deletion_silent
#print axioms Vimanam.History.pre175_enum_introduction_silent
