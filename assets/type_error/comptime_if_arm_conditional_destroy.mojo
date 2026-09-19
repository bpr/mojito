# An arm of a `comptime if` keyed on the body's own parameter may assume
# nothing about the instantiation: the arms join as the branches of an `if`.
# Only `f[Int]` exists, whose arm does close `r`, and the body is rejected all
# the same.
# expect: 'r' abandoned without being explicitly destroyed: close it
@explicit_destroy("close it")
struct R(Deinitable where False):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def close(deinit self):
        print("close", self.x)

def f[T: AnyType]():
    var r = R(1)
    comptime if T == Int:
        r^.close()

def main():
    f[Int]()
