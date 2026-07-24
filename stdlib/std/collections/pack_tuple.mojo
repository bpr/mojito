# Self-hosted variadic tuple prototype (roadmap: variadic-generic
# heterogeneous structs). `PackTuple[*Ts]` reproduces the native tuple
# surface — heterogeneous construction, exact per-index element typing at
# compile-time-constant indices, length, immutability (no `__setitem__`),
# and non-iterability (no `__iter__`) — as an ordinary variadic-generic
# struct. The native-`Tuple` swap is the protocolize-collections milestone;
# native `Tuple[*Ts]` remains the internal heterogeneous storage primitive
# (the analog of Mojo's MLIR pack) behind `storage`.


struct PackTuple[*Ts: Copyable & Movable](Copyable, Movable):
    var storage: Tuple[*Ts]

    def __init__(out self, var *args: *Ts):
        self.storage = Tuple(*args^)

    def __getitem__[i: Int](self) -> Ts[i]:
        return self.storage[i]

    def __len__(self) -> Int:
        return len(self.storage)
