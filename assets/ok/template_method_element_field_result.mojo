# A per-instantiation method clone inherits its checked template's facts when
# it returns a field of a subscripted element as a reference
# (`docs/notes/instantiation-from-template.md`, class MethodBody):
# `return self.entries[i].value`, read and written through, as `Dict`'s own
# `__getitem__` does. The path below the getter's reference is the
# statement's syntax, and the referent's type substitutes.
struct Entry[T: Copyable & Deinitable](Copyable):
    var key: Int
    var value: Self.T

    def __init__(out self, key: Int, var value: Self.T):
        self.key = key
        self.value = value^


struct Table[T: Copyable & Deinitable]:
    var entries: List[Entry[Self.T]]

    def __init__(out self):
        self.entries = List[Entry[Self.T]]()

    def add(mut self, key: Int, var value: Self.T):
        self.entries.append(Entry[Self.T](key, value^))

    def value_at(ref self, i: Int) -> ref[self.entries[0].value] Self.T:
        return self.entries[i].value

    def key_at(self, i: Int) -> ref[self.entries[0].key] Int:
        return self.entries[i].key


def main() raises:
    var t = Table[Int]()
    t.add(1, 10)
    t.add(2, 20)
    ref first = t.value_at(0)
    first = 15
    print(t.value_at(0))
    print(t.value_at(1))
    print(t.key_at(1))
    var s = Table[String]()
    s.add(3, "a")
    s.add(4, "bc")
    ref second = s.value_at(1)
    second += "d"
    print(s.value_at(0))
    print(s.value_at(1))
    print(s.key_at(0))
    var d = Dict[String, Int]()
    d["x"] = 3
    d["y"] = 4
    print(d["x"] + d["y"])
