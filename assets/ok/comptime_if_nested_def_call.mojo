# A generic `def` nested in another body may call a `def` whose `comptime if`
# keys on its own type parameter: each call to the nested `def` specializes it,
# and the specialization reaches the callee's clone. The enclosing body may be
# ordinary code, a generic `def`, or a generic struct's method.

def show[T: Copyable](x: T):
    comptime if T == Int:
        print("int")
    else:
        print("other")

def outer[T: Copyable](x: T):
    def inside[U: Copyable](y: U):
        show(y)
    inside(x)
    inside(1)

struct Box[T: Copyable & Deinitable](Deinitable):
    var x: Self.T

    def __init__(out self, x: Self.T):
        self.x = x.copy()

    def run(self):
        def helper[U: Copyable](y: U):
            show(y)
        helper(self.x)

def main():
    def inner[U: Copyable](y: U):
        show(y)
    inner(2)
    inner(True)
    outer(1.5)
    Box[Int](3).run()
