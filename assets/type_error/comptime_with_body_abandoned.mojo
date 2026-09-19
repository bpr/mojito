# A `with` body inside a compile-time-keyed template is checked abstractly
# like any other block: nothing calls `f`, and the `R` its `with` abandons is
# still reported.
# expect: 'r' abandoned without being explicitly destroyed: close it
@explicit_destroy("close it")
struct R(Deinitable where False):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def close(deinit self):
        print("close", self.x)

struct CM:
    def __init__(out self):
        pass

    def __enter__(self) -> Int:
        return 1

    def __exit__(self):
        print("exit")

def f[T: AnyType]():
    comptime if T == Int:
        with CM() as h:
            var r = R(5)
            print(h)

def main():
    print("unreached by f")
