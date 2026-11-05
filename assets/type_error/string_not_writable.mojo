# `String(x)` of a value that is not `Writable` matches none of the
# bundled initializers.
# expect: no constructor overload matches the supplied arguments
struct Q:
    var x: Int

    def __init__(out self):
        self.x = 1


def main():
    print(String(Q()))
