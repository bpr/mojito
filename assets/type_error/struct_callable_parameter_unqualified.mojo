# A struct's callable parameter is spelled `Self.f` inside its body; the
# bare spelling is the pin's error, called or read.
# expect: unqualified access to struct parameter 'f'; use 'Self.f' instead


def seven() -> Int:
    return 7


struct S[f: def() thin -> Int](Movable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def get(self) -> Int:
        return self.x + f()


def main():
    print(S[seven](1).get())
