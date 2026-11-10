# A struct's callable parameter is part of the instance's identity: the
# instance over `eight` is not the instance over `seven`.
# expect: type mismatch for argument 's' to 'take': expected S[seven], found S[eight]


def seven() -> Int:
    return 7


def eight() -> Int:
    return 8


struct S[f: def() thin -> Int](Movable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x


def take(s: S[seven]):
    print(s.x)


def main():
    take(S[eight](1))
