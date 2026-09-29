# PROBE: a homogeneous `*args` whose element type carries a loan.
#
# **Differs.** The pin prints 4. Mojito rejects the call, "type mismatch for
# variadic argument to 'two$y9:Span[Int]': expected Span[Int], found
# Span[Int]". Filed in `docs/roadmap.md` §3. When Mojito runs it, promote
# this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-09-28 against the pinned Mojo.
#
# Run:    mojo run homogeneous_variadic_loan_carrying_element.mojo
def two[T: Copyable](*args: T):
    pass

def run(xs: List[Int]):
    two(Span(xs), Span(xs))
    print(xs[0])

def main():
    run([4, 5, 6])
