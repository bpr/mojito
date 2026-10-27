# A type pack read as a value is a `TypeList`, so a `comptime` binding that
# annotates it with another type is rejected, as the pin rejects converting
# 'TypeList[Ts.values]' to 'Int'.
# expect: type mismatch for comptime 'y'
def go[*Ts: Movable]():
    comptime y: Int = Ts
    print(y)


def main():
    go[Int, Bool]()
