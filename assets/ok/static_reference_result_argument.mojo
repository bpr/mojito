# A static method's `ref` result read as a call argument or operator operand:
# the call's type receiver is no place, so the setup read roots at `a`.
struct Plain:
    var v: Int

    @staticmethod
    def same(ref x: Int) -> ref[x] Int:
        return x


def show(n: Int):
    print(n)


def main() raises:
    var a = 3
    show(Plain.same(a))
    print(Plain.same(a) + 1)
