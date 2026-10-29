# A written-back subscript's temporary is the getter's owned result: the
# setter takes it by `var`, a `read` operation destroys it after the store,
# and a generic container writes back the same way.


struct P(Copyable, Movable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def __init__(out self, *, copy: Self):
        print("copy", copy.x)
        self.x = copy.x

    def __deinit__(deinit self):
        print("del", self.x)

    def bump(mut self):
        self.x += 1

    def read(self) -> Int:
        return self.x


struct S[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self, var a: Self.T, var b: Self.T):
        self.items = List[Self.T]()
        self.items.append(a^)
        self.items.append(b^)

    def __getitem__(self, i: Int) -> Self.T:
        print("get", i)
        return self.items[i].copy()

    def __setitem__(mut self, i: Int, var value: Self.T):
        print("set", i)
        self.items[i] = value^


def touch[T: Copyable & Deinitable](mut t: T):
    print("touch")


def main():
    var s = S[P](P(1), P(2))
    s[0].bump()
    print(s.items[0].x)
    print(s[1].read())
    for i in range(2):
        if i == 1:
            s[i].x = 40 + i
    print(s.items[1].x)
    touch(s[0])
    print("end")
