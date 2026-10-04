# Pin gap probe (Mojo 1.2.0.dev2026092105): `comptime if conforms_to(T, X):`
# over a plain `def`'s type parameter. The pin prints `copyable`; Mojito
# reports "invalid checked program: fn '$comptime$tag$0': register r0 has no
# checked type". Roadmap R280.
def tag[T: Movable](x: T) -> String:
    comptime if conforms_to(T, Copyable):
        return "copyable"
    else:
        return "move-only"


def main():
    print(tag(1))
