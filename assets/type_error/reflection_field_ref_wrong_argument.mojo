# expect: invalid call to 'field_ref': value passed to 's' cannot be converted from 'Q' to ref 'P'
# `reflect[P].field_ref[idx](s)` takes `ref s: P`: an argument of another
# struct does not bind, as at the pin.
@fieldwise_init
struct P(Copyable):
    var x: Int
    var y: Int


@fieldwise_init
struct Q(Copyable):
    var x: Int


def main():
    var q = Q(1)
    print(reflect[P].field_ref[0](q))
