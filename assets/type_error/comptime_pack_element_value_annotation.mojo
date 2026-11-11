# A type pack's element is a type whose metatype is the pack's trait bound,
# so a `comptime` name annotated `Int` cannot bind it.
# expect: cannot implicitly convert 'Copyable' value to 'Int'
def go[*Ts: Copyable]():
    comptime x: Int = Ts[0]
    pass


def main():
    go[Int, Float64]()
