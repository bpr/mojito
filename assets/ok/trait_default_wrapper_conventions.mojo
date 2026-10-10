# A conformer's inherited default forwards its receiver and every argument
# with the convention it declares: `mut`, `deinit`, and `var` receivers and
# parameters, an explicit value parameter, a raising default, a type pack
# read or owned, and a keyword collector.
trait T:
    def bump(mut self):
        ...

    def twice(mut self):
        self.bump()
        self.bump()

    def take(deinit self, var s: String) -> String:
        return s + "!"

    def p[n: Int](self) -> Int:
        return n

    def r(self, a: Int) raises -> Int:
        if a < 0:
            raise Error("neg")
        return a

    def show[*Ts: Writable](self, *args: *Ts) -> Int:
        comptime for i in range(args.__len__()):
            print(args[i])
        return args.__len__()

    def keep[*Ts: Writable & Movable](self, var *args: *Ts) -> Int:
        return args.__len__()

    def options(self, var **opts: Int) -> Int:
        return len(opts)

struct S(T, Movable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def bump(mut self):
        self.x += 1

def main() raises:
    var s = S(4)
    s.twice()
    print(s.x, s.p[7](), s.r(3))
    try:
        _ = s.r(-1)
    except e:
        print(e)
    print(s.show(1, "a", 2.5), s.keep(1, "b"), s.options(a=1, b=2))
    print(s^.take("hi"))
