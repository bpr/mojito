# A value pack is its `ParameterList` in every position: `size` is the
# list's compile-time member, read through the value as upstream's
# documentation spells it (`args.length`), a runtime `len` or subscript calls
# `__len__` or `__getitem__`, and a compile-time subscript names the element.
# A compile-time `len` of a pack, in a signature or a struct member, is the
# library `len`'s application over the pack's `ParameterList` or `TypeList`.


struct Counter[k: Int]:
    comptime next: Int = Self.k + 1

    def __init__(out self):
        pass


struct Width[n: Int]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.n


struct Measured[*values: Int]:
    comptime size: Int = len(Self.values)


struct Kinds[*Ts: Movable]:
    comptime count: Int = len(Self.Ts)


def counted[*values: Int]() -> Width[len(values)]:
    return {}


def widened[*values: Int]() -> Width[values.length]:
    return Width[values.length]()


def sizes[*values: Int]() -> Int:
    comptime n = values.length
    var t = 0
    comptime for i in range(values.length):
        t += values[i]
    return n * 100 + t


def elements[*values: Int](i: Int):
    comptime second = values[1]
    comptime both = values[0] + second
    print(second, both, materialize[values[2]]())
    print(values[0], len(values), values[i])
    var l = values
    print(l[i], l.length, len(l))


def instance[k: Int]() -> Int:
    var c = Counter[k]()
    return c.next


def main():
    print(sizes[3, 4, 5]())
    elements[7, 8, 9](2)
    print(instance[41]())
    print(widened[1, 2]().get())
    print(Measured[1, 2, 3].size, Kinds[Int, Bool].count)
    print(counted[4, 5, 6, 7]().get())
