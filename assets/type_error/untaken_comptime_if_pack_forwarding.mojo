# expect: expected Int, found StringLiteral
# A body that forwards its pack to another callee is validated from its
# template: the callee's pack binds to the caller's whole pack, so the
# untaken arm after the call is checked and rejected as the pin rejects it.
def inner[*Ts: Writable](*a: *Ts):
    comptime for i in range(a.__len__()):
        print(a[i])


def outer[*Ts: Writable](*a: *Ts):
    inner(*a)
    comptime if 1 > 2:
        var x: Int = "bad"


def main():
    outer(1, "two")
