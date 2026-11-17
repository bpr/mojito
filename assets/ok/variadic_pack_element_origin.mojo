# A `read` collector's element reference names the function's implicit pack
# origin (`args.origin`), as upstream's `VariadicPack[origin=args, ...]` does:
# a `ref` binding of an element holds across other calls, and a function
# returning through one of its own origin binders returns that binder's
# referent beside an unrelated pack.
@fieldwise_init
struct Noisy(Movable, Writable):
    var v: Int

    def __deinit__(deinit self):
        print("del", self.v)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Noisy(", self.v, ")")


def held[*Ts: Writable](*args: *Ts):
    ref r = args[0]
    print("before")
    print(r)


def pick[o: ImmOrigin, *Ts: Writable](
    p: Pointer[Int, o], *args: *Ts
) -> ref[o] Int:
    print(args[1])
    return p[]


def first[*Ts: Writable](*args: *Ts) -> String:
    return String(args[0])


def main():
    var a = Noisy(1)
    var b = Noisy(2)
    held(a, b)
    print("after")
    var x = 5
    print(pick(Pointer(to=x), 7, "a"))
    print(first(3, "b"))
