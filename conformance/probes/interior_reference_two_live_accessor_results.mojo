# PROBE (divergence): two interior references from a `ref self` accessor on
# the same mutable owner, live together in one argument list.
#
# The pinned Mojo treats each call as a mutable borrow of `t`, so the second
# call invalidates the first call's reference before `print` reads it. A
# `List` subscript pair (`print(l[0], l[1])`) is accepted. Mojito tracks no
# conflict between the two accessor results and prints both.
#
# Observed 2026-09-26 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   pin:    error: use of invalidated interior reference 't.entries["element"]'
#           (both `print` lines over `t`)
#   mojito: 1 2, 1 2, 1 2
struct Entry(Copyable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value


struct Table:
    var entries: List[Entry]

    def __init__(out self):
        self.entries = List[Entry]()

    def value_at(ref self, i: Int) -> ref[self.entries[0].value] Int:
        return self.entries[i].value

    def elem_at(ref self, i: Int) -> ref[self.entries[0]] Entry:
        return self.entries[i]


def main():
    var l = List[Int]()
    l.append(1)
    l.append(2)
    print(l[0], l[1])
    var t = Table()
    t.entries.append(Entry(1))
    t.entries.append(Entry(2))
    print(t.elem_at(0).value, t.elem_at(1).value)
    print(t.value_at(0), t.value_at(1))
