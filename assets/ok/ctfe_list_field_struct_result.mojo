@fieldwise_init
struct Bag(Copyable, Movable):
    var xs: List[Int]
    var n: Int

def make() -> Bag:
    var xs = List[Int]()
    xs.append(7)
    return Bag(xs^, 1)

def main():
    comptime b = make()
    print(b.n)
    var c = materialize[b]()
    print(c.xs[0], c.n)
