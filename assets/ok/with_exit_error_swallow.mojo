# A context manager with both `__exit__` overloads: an error raised in the
# body goes to `__exit__(self, err) -> Bool`, whose `True` swallows it and
# whose `False` re-raises it; the plain `__exit__` runs on a normal exit and
# on `return`.
struct Swallow:
    var name: String

    def __init__(out self, name: String):
        self.name = name

    def __enter__(self) -> Int:
        print("enter")
        return 1

    def __exit__(self):
        print("exit plain")

    def __exit__(self, err: Error) -> Bool:
        print("exit err:", String(err))
        return True


struct Propagate:
    def __init__(out self):
        pass

    def __enter__(self) -> Int:
        return 2

    def __exit__(self):
        print("exit plain P")

    def __exit__(self, err: Error) -> Bool:
        print("exit err P:", String(err))
        return False


def go(x: Int) raises -> Int:
    with Swallow("s") as v:
        if x > 0:
            raise Error("boom")
        print("no raise")
    print("after swallow")
    with Propagate() as w:
        if x > 1:
            raise Error("bang")
        return w
    return -1


def main():
    try:
        print(go(0))
        print(go(1))
        print(go(2))
    except e:
        print("caught", String(e))
