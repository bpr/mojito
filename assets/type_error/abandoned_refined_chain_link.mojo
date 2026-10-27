# A chained comparison's middle operand becomes a temporary its second link
# reads. Its type parameter's bound is refined by the `where` clause, so the
# message names the refinement's trait, `Comparable`, which sorts before the
# declared `ImplicitlyCopyable`.
# expect: '(expression temporary)' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'Comparable'
def less[
    T: ImplicitlyCopyable
](a: T, b: T, c: T) -> Bool where conforms_to(T, Comparable):
    return a < b < c


def main():
    print(less(1, 2, 3))
