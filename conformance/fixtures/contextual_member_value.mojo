# mojo-only (strict-subset gap): the audited head resolves a BARE leading-dot
# value member (`takes_color(.red)`) against a struct-body `comptime red =
# Color(1)` associated value and prints 1. Mojito lacks struct comptime
# associated VALUE members entirely (the declaration itself rejects with
# "not a compile-time Int constant"), so the contextual form is recorded as a
# gap alongside parametric statics (`.make[4]()`) and generic expected types.
@fieldwise_init
struct Color(ImplicitlyCopyable, Movable):
    var value: Int

    comptime red = Color(1)

def takes_color(c: Color) -> Int:
    return c.value

def main():
    print(takes_color(.red))
