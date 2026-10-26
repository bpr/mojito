# expect: type mismatch for if condition
# A condition over a pack element converts through `Boolable` only when the
# pack's bounds give it: `Ts: Writable & Defaultable` does not.
def pack_if[*Ts: Writable & Defaultable & Deinitable]():
    comptime for i in range(len(Ts)):
        comptime T = Ts[i]
        var value: T = T()
        if value:
            print(value)


def main():
    pack_if[Int]()
