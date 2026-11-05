# A `read` heterogeneous collector is upstream's `VariadicPack` over the
# arguments' references: each argument is read where it lies, an rvalue
# lives in a slot the caller destroys after the call, and the pack's owners
# die after the call, latest-declared first, as the pin destroys them. The
# body reads `args[i]` through `__getitem_param__`, `args.__len__()` at
# compile time, and forwards the pack to a `VariadicPack` parameter.


@fieldwise_init
struct T(Writable, Movable):
    var v: Int

    def __del__(deinit self):
        print("del", self.v)

    def write_to(self, mut w: Some[Writer]):
        w.write("T", self.v)


def mk(v: Int) -> T:
    return T(v)


def show[*Ts: Writable](*args: *Ts):
    comptime for i in range(args.__len__()):
        print(args[i])


def count[*Ts: Writable](p: VariadicPack[element_trait=Writable, False, *Ts]):
    comptime for i in range(p.__len__()):
        print(i, p[i])
    print("len", len(p))


def forward[*Ts: Writable](*args: *Ts):
    count(args)


def main():
    var a = T(1)
    var b = T(2)
    show(a, mk(3), b, 4, "s")
    print("after show")
    var c = T(5)
    forward(c, mk(6), 7)
    print("after forward")
    print(String(1, "a", 2.5, sep="-", end="!"))
    print("end")
