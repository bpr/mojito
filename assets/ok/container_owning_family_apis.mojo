# The nightly-§6 owning family APIs across the bundled containers:
# linear-capable `deinit_with` on List/Array/Dict/Set/StringDict/Tuple
# (`var`-convention handlers, front-to-back drains), `clear_with` on Dict and
# Set, and displacement-returning `insert` — the displaced ENTRY (key and
# value) on Dict/StringDict, the displaced element on Set.
from std.collections.set import Set
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
    conns^.deinit_with(lambda (var element: Conn): element^.close())

    # Array.deinit_with
    var arr: Array[Res, 2] = [Res(3), Res(4)]
    arr^.deinit_with(lambda (var element: Res): element^.__deinit__())

    # Dict displacement insert (returns the displaced entry) + clear_with
    var d: Dict[Int, Int] = {1: 10}
    var displaced = d.insert(1, 11)
    print("displaced", displaced.value().key, displaced.value().value)
    var fresh = d.insert(2, 20)
    print("fresh", Bool(fresh))
    d.clear_with(lambda (var key: Int, var value: Int): print("cleared", key, value))
    print("len after clear", len(d))
    d[5] = 50
    d^.deinit_with(lambda (var key: Int, var value: Int): print("torn", key, value))

    # Set displacement insert + clear_with + deinit_with
    var s: Set[Int] = {7}
    print("set displaced", s.insert(7).or_else(-1))
    print("set fresh", Bool(s.insert(8)))
    s.clear_with(lambda (var element: Int): print("s cleared", element))
    s.add(9)
    s^.deinit_with(lambda (var element: Int): print("s torn", element))

    # StringDict insert (displaced entry) + deinit_with
    var sd = StringDict[Int]()
    sd["a"] = 1
    var sd_displaced = sd.insert("a", 2)
    print("sd displaced", sd_displaced.value().value)
    print("sd fresh", Bool(sd.insert("b", 3)))
    sd^.deinit_with(lambda (var key: StringLiteral, var value: Int): print("sd torn", key, value))

    # Tuple.deinit_with (consume_elements family spelling)
    var t = ([Res(5)], 6)

    @parameter
    def toss[index: Int](var element: t.element_types[index]):
        pass

    t^.deinit_with[toss]()
    print("done")
