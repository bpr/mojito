# A name inside a `try` region is the binding its scope declares: an
# assignment there writes the inner of two same-spelled variables, each
# `except` name is its own variable, and a second `with … as h` target is
# not the first.
struct Guard(Movable):
    var name: String

    def __init__(out self, name: String):
        self.name = name

    def __enter__(var self) -> Self:
        return self^

    def __deinit__(deinit self):
        print("del", self.name)


def boom(n: Int) raises -> Int:
    if n > 0:
        raise Error("boom" + String(n))
    return n


def shadowed_assignment():
    var h = 1
    if True:
        var h = 2
        try:
            h = 5
            print(h)
        except e:
            print(e)
        print(h)
    print(h)


def sequential_handlers():
    try:
        _ = boom(1)
    except e:
        print("first", e)
    try:
        _ = boom(2)
    except e:
        print("second", e)
        try:
            _ = boom(3)
        except e:
            print("third", e)
        print("second again", e)


def sequential_with_targets():
    with Guard("a") as h:
        print("body", h.name)
    with Guard("b") as h:
        h.name = "c"
        print("second h", h.name)


def main():
    shadowed_assignment()
    sequential_handlers()
    sequential_with_targets()
