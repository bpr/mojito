# A view returned by a method on a container element borrows that element's
# owned interior: the container stays alive for the view's whole life, sibling
# element reads coexist with it, and the element may be replaced once the
# view's last use has passed.
struct Named(Copyable, Movable):
    var name: String

    def __init__(out self, name: String):
        self.name = name

struct Bag(Movable):
    var items: List[String]

    def __init__(out self):
        self.items = [String("m  "), String("n ")]

    def joined(self) -> String:
        var v = self.items[0].rstrip()
        var u = self.items[1].rstrip()
        return String(v) + String(u)

    def mark(mut self):
        var v = self.items[0].rstrip()
        var s = String(v)
        self.items[0] = s + "!"

def main() raises:
    var ys: List[String] = [String("q  "), String("r ")]
    var v = ys[0].rstrip()
    var z = String(v)
    print(z)

    var a = ys[0].rstrip()
    var b = ys[1].rstrip()
    print(ys[1], len(ys))
    print(String(a), String(b))
    ys[0] = String("zz")
    ys.append(String("k"))
    print(ys[0], len(ys))

    var total = 0
    for i in range(len(ys)):
        var w = ys[i].rstrip()
        total += w.byte_length()
    print(total)

    var yss: List[List[String]] = [[String("a "), String("b  ")]]
    var inner = yss[0][1].rstrip()
    print(String(inner))

    var ps: List[Named] = [Named(String("n  ")), Named(String("m "))]
    var first = ps[0].name.rstrip()
    var second = ps[1].name.rstrip()
    print(ps[1].name)
    print(String(first), String(second))

    var d: Dict[String, String] = {}
    d["k"] = String("val  ")
    var dv = d["k"].rstrip()
    print(String(dv))

    var bag = Bag()
    print(bag.joined())
    bag.mark()
    print(bag.items[0])
