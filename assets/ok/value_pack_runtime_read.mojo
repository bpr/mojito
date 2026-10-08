# A value pack read as a runtime value is the zero-sized `ParameterList` the
# pin types it as: `for` iterates it, a runtime index reads the element from
# the list's static constant array, and the whole pack binds as one value.


def total[*values: Int]() -> Int:
    var s = 0
    for v in values:
        s += v
    return s


def at[*values: Int](i: Int) -> Int:
    return values[i]


def whole[*values: Int]() -> Int:
    var l = values
    return len(l) + l[0]


def flags[*fs: Bool]() -> Int:
    var n = 0
    for f in fs:
        if f:
            n += 1
    return n


def kinds[*ks: DType]() -> Int:
    var n = 0
    for k in ks:
        if k == DType.float32:
            n += 1
    return n


def floats[*xs: Float64]() -> Float64:
    var t = 0.0
    for x in xs:
        t += x
    return t


def indexed[*values: Int]() -> Int:
    var t = 0
    for i in range(len(values)):
        t += values[i]
    return t


# A body the elaborator still clones reads its pack the same way.
def cloned[*values: Int](i: Int) -> Int:
    var t = 0
    comptime for x in [values[0] + 1, 7]:
        t += x
    return t + values[i]


struct Holder:
    def __init__(out self):
        pass

    def sum[*vs: Int](self) -> Int:
        var t = 0
        for v in vs:
            t += v
        return t


struct Pack[*values: Int](Sized):
    def __init__(out self):
        pass

    def __len__(self) -> Int:
        return len(Self.values)

    def total(self) -> Int:
        var t = 0
        for v in Self.values:
            t += v
        return t

    def at(self, i: Int) -> Int:
        return Self.values[i]


def main():
    print(total[1, 2, 3]())
    print(at[4, 5, 6](1))
    print(whole[7, 8]())
    print(flags[True, False, True]())
    print(kinds[DType.float32, DType.int8, DType.float32]())
    print(floats[1.5, 2.25]())
    print(cloned[1, 2](1))
    print(Holder().sum[4, 5]())
    var p = Pack[10, 20, 30]()
    print(p.total(), p.at(2), len(p))
    print(indexed[3, 1, 4]())
