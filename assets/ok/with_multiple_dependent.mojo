# Several comma-separated context managers: a later item's expression sees
# an earlier item's binding, exits run innermost-first, a plain `__exit__`
# runs after a `break`, and an unbound item still enters and exits.
struct Counter:
    var name: String
    var count: Int

    def __init__(out self, name: String, count: Int):
        self.name = name
        self.count = count

    def __enter__(self) -> Int:
        print("enter", self.name)
        return self.count

    def __exit__(self):
        print("exit", self.name)

    def __deinit__(deinit self):
        print("del", self.name)


def main():
    with Counter("outer", 2) as n, Counter("inner", n * 10) as m:
        print("body", n, m)
    print("after")
    with Counter("x", 1), Counter("y", 2) as k:
        print("body", k)
    for i in range(2):
        with Counter("loop", i) as j:
            print("loop", j)
            break
    print("done")
