# The nightly-§6 owning family APIs across the bundled containers:
# linear-capable `deinit_with` on List/Array/Dict/Set/StringDict/Tuple,
# `clear_with` on Dict and Set, and displacement-returning `insert` on
# Dict, Set, and StringDict.
from std.collections.string_dict import StringDict

struct Res(Movable):
    var id: Int
    def __init__(out self, id: Int):
        self.id = id
    def __init__(out self, *, deinit move: Self):
        self.id = move.id
    def __deinit__(deinit self):
        print("drop", self.id)

@explicit_destroy("close Conn")
struct Conn(Movable, Deinitable where False):
    var id: Int
    def __init__(out self, id: Int):
        self.id = id
    def close(deinit self):
        print("close", self.id)

def main():
    # List.deinit_with over linear elements — the §6 payoff
    var conns: List[Conn] = [Conn(1), Conn(2)]
    conns^.deinit_with(lambda (deinit element: Conn): element^.close())

    # Array.deinit_with
    var arr: Array[Res, 2] = [Res(3), Res(4)]
    arr^.deinit_with(lambda (deinit element: Res): element^.__deinit__())

    # Dict displacement insert + clear_with
    var d: Dict[Int, Int] = {1: 10}
    var displaced = d.insert(1, 11)
    print("displaced", displaced.or_else(-1))
    var fresh = d.insert(2, 20)
    print("fresh", fresh.is_some())
    d.clear_with(lambda (deinit key: Int, deinit value: Int): print("cleared", key, value))
    print("len after clear", len(d))
    d[5] = 50
    d^.deinit_with(lambda (deinit key: Int, deinit value: Int): print("torn", key, value))

    # Set displacement insert + clear_with + deinit_with
    var s: Set[Int] = {7}
    print("set displaced", s.insert(7).or_else(-1))
    print("set fresh", s.insert(8).is_some())
    s.clear_with(lambda (deinit element: Int): print("s cleared", element))
    s.add(9)
    s^.deinit_with(lambda (deinit element: Int): print("s torn", element))

    # StringDict insert + deinit_with
    var sd = StringDict[Int]()
    sd["a"] = 1
    print("sd displaced", sd.insert("a", 2).or_else(-1))
    print("sd fresh", sd.insert("b", 3).is_some())
    sd^.deinit_with(lambda (deinit key: StringLiteral, deinit value: Int): print("sd torn", key, value))

    # Tuple.deinit_with (consume_elements family spelling)
    var t = ([Res(5)], 6)

    @parameter
    def toss[index: Int](var element: t.element_types[index]):
        pass

    t^.deinit_with[toss]()
    print("done")
