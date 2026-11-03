# expect: expression must be mutable in assignment
# A field reference has its argument's mutability: through a read argument
# it is immutable, so a store through it is rejected, as at the pin.
@fieldwise_init
struct P(Copyable):
    var x: Int
    var y: Int


def set(p: P):
    reflect[P].field_ref[0](p) = 1


def main():
    set(P(1, 2))
