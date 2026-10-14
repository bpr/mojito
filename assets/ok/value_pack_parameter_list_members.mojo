# A value pack is its `ParameterList` in every position: `size` is the
# list's compile-time member, read through the value as upstream's
# documentation spells it (`args.size`), a runtime `len` or subscript calls
# `__len__` or `__getitem__`, and a compile-time subscript names the element.


struct Counter[k: Int]:
    comptime next: Int = Self.k + 1

    def __init__(out self):
        pass


struct Width[n: Int]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.n


def widened[*values: Int]() -> Width[values.size]:
    return Width[values.size]()


def sizes[*values: Int]() -> Int:
    comptime n = values.size
    var t = 0
    comptime for i in range(values.size):
        t += values[i]
    return n * 100 + t


def elements[*values: Int](i: Int):
    comptime second = values[1]
    comptime both = values[0] + second
    print(second, both, materialize[values[2]]())
    print(values[0], len(values), values[i])
    var l = values
    print(l[i], l.size, len(l))


def instance[k: Int]() -> Int:
    var c = Counter[k]()
    return c.next


def main():
    print(sizes[3, 4, 5]())
    elements[7, 8, 9](2)
    print(instance[41]())
    print(widened[1, 2]().get())
