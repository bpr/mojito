# A compile-time list read at an `Array` is one frozen value, materialized at
# each read without running an element initializer: each read holds what a
# run-time display of the same elements holds, however often it is read.
struct M(Copyable, Movable, Writable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100

    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1000

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self.v)


comptime L: Array[M, 2] = [M(1), M(2)]
comptime U = [M(3), M(4)]


def same(got: Array[M, 2], built: Array[M, 2]) -> Bool:
    return got[0].v == built[0].v and got[1].v == built[1].v


def main():
    var built: Array[M, 2] = [M(1), M(2)]
    var a = materialize[L]()
    var b = materialize[L]()
    print(same(a, built), same(b, built))
    var u = materialize[U]()
    var built_u: Array[M, 2] = [M(3), M(4)]
    print(same(u, built_u), u[1].v - u[0].v)
