# PROBE (upstream regression): a `var` binding beside a literal in a tuple
# pattern over an rvalue subject.
#
# Found at the 2026-11-01 re-pin. `Mojo 1.2.0.dev2026101705` and Mojito print
# `hit 1`, `named 2`, and `match 3`. `Mojo 1.3.0.dev2026110105 (ce3afc14)`
# rejects the first and third ("use of uninitialized value '(pattern
# child)'"), the window that taught a `var` case binding to take its payload
# from an rvalue subject. A named subject still runs.
#
# Three fixtures sit in `conformance/assets-mojo-rejects.tsv` as
# `upstream-regression` for it: `if_case_and_while_case.mojo`,
# `if_case_mixed_with_boolean_elif.mojo`, and
# `match_struct_and_initializer_patterns.mojo`. Delete their rows and this
# probe when the pin runs it again.
#
# Run:    mojo run var_case_rvalue_tuple_literal_sibling.mojo
#         cargo run -- run conformance/probes/var_case_rvalue_tuple_literal_sibling.mojo
def get(n: Int) -> Tuple[Int, Int]:
    return (n, 0)


def main():
    if case (var x, 0) = get(1):
        print("hit", x)
    var t = get(2)
    if case (var y, 0) = t:
        print("named", y)
    match get(3):
        case (var z, 0):
            print("match", z)
        case _:
            print("miss")
