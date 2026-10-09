# expect: rebind input type 'String' does not match result type 'S'
# A projection after a `rebind` is still judged per instance: `get[String]`
# reads field `f` of `S` through a `String`, so its instantiation fails, as
# at the pin.


@fieldwise_init
struct S(Copyable):
    var f: Int


def get[T: Copyable](x: T) -> Int:
    return rebind[S](x).f


def main():
    print(get(String("four")))
