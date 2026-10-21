# An owned tuple yield lives in the loop's hidden per-iteration binder,
# its leaves borrow it, and it is destroyed after its leaves' last use,
# as on the pin, across `continue`, `break`, and sequence targets.
@fieldwise_init
struct D(Copyable):
    var v: Int
    def __deinit__(deinit self):
        print("del", self.v)

struct It(Iterator, Copyable):
    comptime Element = Tuple[D, D]
    var i: Int
    def __init__(out self):
        self.i = 0
    def __next__(mut self) raises StopIteration -> Self.Element:
        if self.i >= 3:
            raise StopIteration()
        self.i += 1
        print("next", self.i)
        return (D(self.i), D(self.i * 10))
    def __iter__(ref self) -> Self:
        return self.copy()

def main() raises:
    for a, b in It():
        if a.v == 2:
            print("continue")
            continue
        print("body", a.v, b.v)
    print("---")
    for a, _ in It():
        if a.v == 2:
            print("break")
            break
        print("body", a.v)
    print("---")
    var rows = List[List[D]]()
    rows.append([D(1), D(2)])
    rows.append([D(3)])
    for [x, *rest] in rows:
        print("seq", x.v, len(rest))
    print("end")
