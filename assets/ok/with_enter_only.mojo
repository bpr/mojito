# A context manager with a consuming `__enter__` and no `__exit__` (the
# `FileHandle` shape): the enter result stands in for the manager and lives
# to the end of the block, bound or not, on normal exit, `return`, `break`,
# and `continue`; the `as` name is block-scoped.
struct Guard(Movable):
    var name: String

    def __init__(out self, name: String):
        self.name = name

    def __enter__(var self) -> Self:
        print("enter", self.name)
        return self^

    def __deinit__(deinit self):
        print("del", self.name)


def shout(text: String) -> String:
    with Guard("ret") as g:
        return text + "!"


def main():
    with Guard("a") as h:
        print("body", h.name)
        print("mid")
    print("after a")
    with Guard("b"):
        print("body b")
    print("after b")
    print(shout("hello"))
    for i in range(3):
        with Guard(String("loop") + String(i)) as g:
            if i == 0:
                continue
            print("loop body", i)
            if i == 1:
                break
    print("after loop")
    with Guard("c") as h:
        print("second h", h.name)
