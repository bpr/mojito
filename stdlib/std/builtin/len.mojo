# Upstream's `std/builtin/len.mojo` traits: `Sized` and `SizedRaising`, each
# requiring `__len__`. The prelude exports both, as upstream's does. `len`
# itself is still a checker builtin (R988).


trait Sized:
    def __len__(self) -> Int:
        ...


trait SizedRaising:
    def __len__(self) raises -> Int:
        ...
