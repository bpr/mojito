# A per-instantiation method clone inherits its checked template's facts when
# the body calls a method whose callee stores an argument outward
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `REPLAYED_TRANSFERS`). The template, whose parameter may stand for a type
# that carries a loan, replays `List.append`'s transfer summary at the call and
# records a transfer, a merged origin, and an effect of its own, each kept by
# the binding it is rooted at. A transfer moves the loans its source carries,
# and every value of the instances below is plain data, so the replay for each
# instance keeps no source: the derived facts hold nothing at the call, and
# the instance's frame publishes nothing, as their own checks record.
struct Bag[T: Copyable & Deinitable](Movable):
    var item: Self.T
    var items: List[Self.T]

    def __init__(out self, var item: Self.T):
        self.item = item^
        self.items = List[Self.T]()

    def push(mut self, var value: Self.T):
        self.items.append(value^)

    def push_first(mut self, var value: Self.T):
        self.items.insert(0, value^)

    def replace(mut self, var value: Self.T):
        self.items[0] = value^

    def count(self) -> Int:
        return len(self.items)


def main():
    var a = Bag[Int](1)
    var b = Bag[String](String("x"))
    a.push(2)
    a.push(3)
    a.push_first(1)
    a.replace(9)
    b.push(String("y"))
    b.push(String("z"))
    b.push_first(String("w"))
    b.replace(String("v"))
    print(a.count(), b.count(), a.items[0], a.items[2], b.items[0], b.items[2])
