# A direct `ref` field of a `Bool` read as a value: `not self.flag` and
# `if self.flag:` test the referent, not the reference.
struct BoolView[o: Origin[mut=False]]:
    var flag: ref[Self.o] Bool

    def __init__(out self, ref [Self.o] flag: Bool):
        self.flag = flag

    def inverted(self) -> Bool:
        return not self.flag

    def tested(self) -> Bool:
        if self.flag:
            return True
        return False


def main():
    var flag = True
    print(BoolView(flag).inverted(), BoolView(flag).tested())
    flag = False
    print(BoolView(flag).inverted(), BoolView(flag).tested())
