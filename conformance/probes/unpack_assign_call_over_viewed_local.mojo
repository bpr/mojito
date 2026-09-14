# Question: `a, b = pair(a.rstrip())`, unpacking a call whose argument views
# the unpacked-into variable.
# Upstream (`1.1.0.dev2026082605`) runs it: `ab 1`. The call-result aliasing
# rule does not apply to unpacking targets there.
#
# Mojito rejects it: "access to 'a' conflicts with live reference
# '$arg_loan_r5'" — the argument's view anchor stays live until the end of
# the statement, past the unpacking store.
#
# On the fix: promote this file to `assets/ok/unpack_assign_call_over_viewed_local.mojo`
# with its manifest rows, and delete the `unpack-assign-call-over-viewed-local`
# ledger row in docs/roadmap.md.
def pair(v: StringSpan) -> Tuple[String, Int]:
    return (String(v), 1)

def main():
    var a = String("ab  ")
    var b = 0
    a, b = pair(a.rstrip())
    print(a, b)
