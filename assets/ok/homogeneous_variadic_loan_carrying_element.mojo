# A homogeneous `*args` whose element type carries a loan: the collected
# arguments bind the element's origin, so `Span(xs)` passes whether `xs` is a
# read parameter or a local, and a result spelled with the element type
# carries the bound origin back to the caller.


def count[T: Copyable](*args: T) -> Int:
    return len(args)


def first[T: Copyable](*args: T) -> T:
    return args[0].copy()


def run(xs: List[Int]):
    print(count(Span(xs), Span(xs)))
    var s = first(Span(xs), Span(xs))
    print(s[1])
    print(xs[0])


def main():
    run([4, 5, 6])
    var ys: List[Int] = [7, 8]
    print(count(Span(ys)))
    print(first(Span(ys))[0])
