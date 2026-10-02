# expect: cannot be implicitly copied
# A place passed to a static method's `var` parameter is copied, so its type
# must be `ImplicitlyCopyable`, as at an instance method's `var` argument:
# transfer it with `^` or spell `.copy()`.
struct Box[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    @staticmethod
    def keep(var v: Self.T) -> Int:
        return 1

    def via_static(self) -> Int:
        return Box.keep(self.item)


def main():
    print(Box[String]("s").via_static())
