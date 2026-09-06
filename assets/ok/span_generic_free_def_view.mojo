# A generic free function returning an origin-bearing struct
# (`-> Span[T, origin_of(xs)]`): each per-instantiation clone keeps the
# return annotation's origin argument (accepted syntactically and erased, as
# the template's is), and the result loans its parameter place exactly like
# the non-generic spelling.
def view[T: Copyable & Movable](ref xs: List[T]) -> Span[T, origin_of(xs)]:
    return Span(xs)

def total(s: Span[Int, _]) -> Int:
    var acc = 0
    for x in s:
        acc += x
    return acc

def main():
    var xs: List[Int] = [1, 2, 3]
    var v = view(xs)
    print(len(v), v[1], total(v))
    var names: List[String] = [String("ada"), String("bob")]
    var w = view(names)
    print(w[0], w[1])
