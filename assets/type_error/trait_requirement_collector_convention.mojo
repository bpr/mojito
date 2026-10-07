# expect: does not match the signature required by trait
# A witness must take its collector by the requirement's convention.
trait Eater:
    def eat[*Ts: Copyable & Writable](self, *a: *Ts):
        ...


struct C(Eater):
    def __init__(out self):
        pass

    def eat[*Ts: Copyable & Writable](self, var *a: *Ts):
        comptime for i in range(a.__len__()):
            print(a[i])


def main():
    C().eat(1)
