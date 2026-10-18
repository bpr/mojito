# PROBE (re-probe): does `Tuple[*Ts](*args^)` — the tuple type spelled with
# an explicit pack argument — construct from a bounded runtime pack?
#
# Upstream regression found at the 2026-10-17 re-pin. Mojo
# 1.2.0.dev2026092105 accepted both spellings; Mojo 1.2.0.dev2026101705
# (46f49943), whose `Tuple` is now `Tuple[*Ts: AnyType]`, rejects the
# explicit one for every bound tried (`Movable`, `Movable & Deinitable`,
# `AnyType`): "cannot unpack a pack of type 'Movable' ('Ts.values') into a
# call that expects a pack of type 'AnyType' ('Ts.values')". The inferred
# spelling `Tuple(*args^)` still runs, so every fixture and the bundled
# `std.collections.pack_tuple` were respelled to it at the re-pin. Mojito
# accepts both.
#
# Expected on both once upstream recovers: prints `2` twice.
#
# Run:    mojo run tuple_explicit_pack_application.mojo
#         cargo run -- run conformance/probes/tuple_explicit_pack_application.mojo

def explicit[*Ts: Movable](var *args: *Ts) -> Tuple[*Ts]:
    return Tuple[*Ts](*args^)


def inferred[*Ts: Movable](var *args: *Ts) -> Tuple[*Ts]:
    return Tuple(*args^)


def main():
    print(len(explicit(1, "two")))
    print(len(inferred(1, "two")))
