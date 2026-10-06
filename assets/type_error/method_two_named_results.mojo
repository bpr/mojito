# expect: multiple named 'out' results
# A method declares at most one named `out` result, as a function does.
@fieldwise_init
struct Pair(Movable):
    var a: Int
    var b: Int

    def both(self, out first: Int, out second: Int):
        first = self.a
        second = self.b


def main():
    var p = Pair(1, 2)
    print(p.both())
