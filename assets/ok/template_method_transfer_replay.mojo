# A checked template whose body replays a callee's transfer summary is
# reused across every transfer round, and its instances replay the same
# transfers on their own bindings (`docs/notes/instantiation-from-template.md`,
# class MethodBody, feature `REPLAYED_TRANSFERS`, obligation 14). The template
# retains each replayed transfer, the origins it merged, and the effect its
# frame derived, every source by the binding it is rooted at: a `var`
# parameter moved into `self`, and a local moved into `self`. An instance
# keeps a source whose binding may still carry a loan and drops one whose
# binding is plain data, so the instances below record nothing at the calls,
# as their own checks would.
struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def push(mut self, var value: Self.T):
        self.items.append(value^)

    def push_held(mut self, var value: Self.T):
        var held = value^
        self.items.append(held^)

    def count(self) -> Int:
        return len(self.items)


def main():
    var a = Bag[Int]()
    a.push(1)
    a.push_held(2)
    var s = Bag[String]()
    s.push(String("x"))
    s.push_held(String("y"))
    print(a.count(), s.count(), a.items[1], s.items[0])
