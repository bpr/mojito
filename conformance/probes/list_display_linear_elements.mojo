# PROBE: a list display of elements that are not `Deinitable`.
#
# **Differs.** The pin runs it and prints `2`, `close 1`, `close 2`: its
# list literal initializer hands the owned variadic to
# `values^.consume_elements(init_elt)`, which never instantiates the
# list's `__deinit__`. Mojito rejects it, "function instantiation of
# `VariadicList.__deinit__` failed: constraint failed": the bundled
# initializer moves the elements out with `for var value in values^` (R32),
# whose narrowed list is destroyed by `__deinit__`, and its `comptime
# assert conforms_to(Self.element_type, Deinitable)` fails. Upstream's
# shape needs a capture-list closure in the bundled library, which the
# package cannot hold yet. Filed in `docs/roadmap.md`. When Mojito runs it,
# promote this file to `assets/ok/`.
#
# Observed 2026-10-28 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run list_display_linear_elements.mojo


@explicit_destroy("close Conn")
struct Conn(Movable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        print("close", self.id)


def close_conn(var c: Conn):
    c^.close()


def main():
    var conns: List[Conn] = [Conn(1), Conn(2)]
    print(len(conns))
    conns^.deinit_with(close_conn)
