# Upstream's `std/builtin/int.mojo`, holding its traits: `Int` is a compiler
# scalar. The prelude exports `Indexer`, `Intable`, and `IntableRaising`, as
# upstream's does.


trait Indexer:
    def __mlir_index__(self) -> __mlir_type.index:
        ...


trait Intable:
    def __int__(self) -> Int:
        ...


trait IntableRaising:
    def __int__(self) raises -> Int:
        ...
