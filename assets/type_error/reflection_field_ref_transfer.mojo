# expect: expression does not designate a value with an origin
# A field reference is an lvalue no tracked root holds, so `^` cannot move
# out of it, as at the pin.
@fieldwise_init
struct P(Movable):
    var x: Int
    var s: String


def main():
    var p = P(1, "a")
    var t = reflect[P].field_ref[1](p)^
    print(t)
