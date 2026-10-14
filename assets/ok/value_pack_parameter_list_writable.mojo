# A homogeneous value pack is its `ParameterList`, which conforms to
# `Writable` where its element type does: it writes as upstream's tuple-like
# `(a, b)`, and its repr names the list and its element type.
def show[*vs: Int]():
    print(vs)
    print(repr(vs))
    var s = String()
    vs.write_to(s)
    print(s, s.byte_length())


def flags[*bs: Bool]():
    print(bs)
    print(String(bs))
    print(repr(bs))


struct Holder[*vs: Int]:
    def __init__(out self):
        pass

    def show(self):
        print(Self.vs)


def main():
    show[1, 2, 3]()
    show[42]()
    flags[True, False]()
    Holder[7, 8]().show()
