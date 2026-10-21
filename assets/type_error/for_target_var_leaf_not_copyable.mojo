# expect: value of type 'C' cannot be implicitly copied
# A `var` leaf copies its piece out of the bound element, so the piece must copy implicitly.
struct C(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def main():
    var xs = List[Tuple[C, Int]]()
    xs.append((C(1), 2))
    for (var a, b) in xs:
        print(a.v, b)
