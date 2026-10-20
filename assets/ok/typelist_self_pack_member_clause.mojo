# A struct member's `where` clause over `TypeList[Self.Ts.values]()` binds
# the bundled struct, and each call decides it at the receiver's pack.
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
