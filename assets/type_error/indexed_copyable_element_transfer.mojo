# expect: expression does not designate a value with an origin
# A subscript yields a reference, which reaches no tracked root. Only a
# trivial register element (`Int`) transfers, as a copy; an implicitly
# copyable struct element does not.
@fieldwise_init
struct P(ImplicitlyCopyable):
    var a: Int
    var b: Int


def first(x: List[P]) -> P:
    return x[0]^


def main():
    print(first([P(1, 2)]).a)
