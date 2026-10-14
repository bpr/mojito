# expect: cannot materialize comptime value of type 'Pair'
# A local `comptime` binding of an application that is not implicitly
# copyable is an alias of the application, as a module constant is: a field
# read is a compile-time projection and crosses, and a whole read
# materializes the value, which the pin rejects.
@fieldwise_init
struct Pair(Copyable, Movable):
    var a: Int
    var b: String


def pair_of(n: Int) -> Pair:
    return Pair(n, String(n))


def main():
    comptime p = pair_of(4)
    print(p.a)
    var x = p
    print(x.b)
