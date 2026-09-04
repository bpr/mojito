# expect: transfer
# `or_else` consumes the Optional (`deinit self`), so a non-ImplicitlyCopyable
# receiver must be transferred explicitly.
@fieldwise_init
struct Res(Copyable, Movable):
    var id: Int

def main():
    var held = Optional[Res](Res(1))
    var taken = held.or_else(Res(2))
    print(taken.id)
