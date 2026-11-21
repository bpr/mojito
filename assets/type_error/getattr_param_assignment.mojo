# A name only `__getattr_param__` answers reads the hook call's value, which
# is no place, so assigning to it is upstream's rejection.
# expect: expression must be mutable in assignment
struct S:
    var x: Int

    def __init__(out self):
        self.x = 1

    def __getattr_param__[name: StaticString](self) -> Int:
        return 5


def main():
    var s = S()
    s.y = 3
    print(s.x)
