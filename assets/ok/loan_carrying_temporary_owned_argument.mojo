# A heap-owning temporary that carries a loan, handed to an owning
# parameter: the anchor keeping `xs` alive is the temporary's call-result
# slot, which the call takes, so the callee is its one owner and destroys
# the list inside it once. Prints 1 and 1, as at the pin.
struct Maybe[T: Copyable & Deinitable](Copyable, Movable):
    var data: List[Self.T]

    def __init__(out self, var value: Self.T):
        self.data = List[Self.T]()
        self.data.append(value^)

    def count(self) -> Int:
        return len(self.data)


def take[T: Copyable & Deinitable](var m: Maybe[T]) -> Int:
    return m.count()


def run(xs: List[Int]):
    print(take(Maybe(Span(xs))))
    var l = List[Maybe[Span[Int, origin_of(xs)]]]()
    l.append(Maybe(Span(xs)))
    print(l[0].count())


def main():
    run([1, 2, 3])
