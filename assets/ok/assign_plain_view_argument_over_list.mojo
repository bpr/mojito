# A temporary plain-origin `Span(xs)` passed to a call whose result is
# assigned back to `xs`. The span borrows `xs` itself, not an owned
# interior, and a store into `xs` beside the loan runs, as at the pin: `1`.
def rebuild(v: Span[Int, _]) -> List[Int]:
    return [v[0], v[0]]

def main():
    var xs: List[Int] = [1, 2]
    xs = rebuild(Span(xs))
    print(xs[1])
