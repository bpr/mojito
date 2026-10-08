# expect: cannot materialize comptime value of type 'Q'
# A function body's whole read of a module struct constant that is not
# implicitly copyable materializes it, which the pin rejects; a field read
# is a compile-time projection and crosses.
@fieldwise_init
struct Q(Copyable, Movable):
    var a: Int
    var s: String

def mk(n: Int) -> Q:
    return Q(n, String(n) + "!")

comptime q0 = mk(3)

def main():
    print(q0.s)
    var x = q0
    print(x.s)
