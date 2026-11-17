# A signature names a `read` collector's pack origin as upstream spells it,
# `ref[args.origin]` or `ref[type_of(args).origin]`, and each call binds it
# to the places the collector gathers: the returned reference reads its
# element after the call, from a free function, a static method, an
# instance method, a clause joining another origin, or a function forwarding
# its own pack's elements to another.
@fieldwise_init
struct Picker(ImplicitlyCopyable):
    @staticmethod
    def first[*Ts: Writable](*args: *Ts) -> ref[args.origin] Ts[0]:
        return args[0]

    def second[*Ts: Writable](self, *args: *Ts) -> ref[args.origin] Ts[1]:
        return args[1]


def first[*Ts: Writable](*args: *Ts) -> ref[args.origin] Ts[0]:
    return args[0]


def last[*Ts: Writable](*args: *Ts) -> ref[type_of(args).origin] Ts[1]:
    return args[1]


def either[*Ts: Writable](ref a: Int, *args: *Ts) -> ref[a, args.origin] Int:
    return a


def outer[*Us: Writable](*args: *Us) -> ref[args.origin] Us[0]:
    return first(args[0], args[1])


def main():
    var x = 1
    var s = String("a")
    print(first(x, s))
    print(last(x, s))
    ref r = first(x, s)
    print(r)
    var y = 2
    var copied = first(y, s)
    print(copied)
    var z = 3
    print(Picker.first(z, s))
    var w = 4
    print(Picker().second(s, w))
    var v = 5
    print(either(v, x, s))
    print(first(String("t"), s))
    var u = 6
    print(outer(u, s))
