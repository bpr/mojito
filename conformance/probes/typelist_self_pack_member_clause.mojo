# Pin gap probe (Mojo 1.6.0.dev2026092105): a struct member's `where` clause
# over `TypeList[Self.Ts.values]()`. The pin prints `1 True 2`; Mojito
# rejects the program with "TypeList[...] takes a pack projection
# ('Ts.values')". Roadmap section 3 carries the entry.
from std.traits import IsTriviallyCopyable


struct Row[*Ts: AnyType]:
    def __init__(out self):
        pass

    def plain(self) -> Int where TypeList[Self.Ts.values]().all[IsTriviallyCopyable]():
        return 1

    def has_int(self) -> Bool where TypeList[Self.Ts.values]().contains[Int]():
        return True

    def pair(self) -> Int where TypeList[Self.Ts.values]().length == 2:
        return 2


def main():
    var r = Row[Int, Bool]()
    print(r.plain(), r.has_int(), r.pair())
