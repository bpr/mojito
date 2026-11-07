# `Error` is upstream's library struct: its `@implicit` initializers from a
# string literal, a `String`, and a variadic `Writable` sequence, its
# declared `write_to` and `write_repr_to`, and a raised string's
# conversion through the initializer.


def show[T: Writable](x: T) -> String:
    var text = String()
    x.write_to(text)
    text += " / "
    x.write_repr_to(text)
    return text


def check(n: Int) raises -> Int:
    if n > 2:
        raise "too big"
    if n < 0:
        raise Error("negative: ", n)
    if n == 0:
        var message = String("zero")
        raise message
    return n


def main():
    var literal = Error("boom")
    var joined = Error("Index ", 3, " not in *args")
    print(literal, joined)
    print(repr(literal))
    print(show(joined))
    var copied = literal
    print(copied, String(copied))
    for n in [5, -1, 0, 1]:
        try:
            print("ok", check(n))
        except e:
            print("caught", e, repr(e))
