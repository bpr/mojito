# Compatibility wrapper for the earlier Mojito prototype.  New tuple displays
# resolve to `std.collections.tuple.Tuple`; explicit PackTuple imports continue
# to work during the migration.

from std.collections.tuple import Tuple

struct PackTuple[*Ts: Copyable & Movable](Copyable, Movable):
    var storage: Tuple[*Ts]

    def __init__(out self, var *args: *Ts):
        self.storage = Tuple[*Ts](*args^)

    def __getitem__[index: Int](self) -> Ts[index]:
        return self.storage[index]

    def __getitem_param__[index: Int](self) -> Ts[index]:
        return self.storage[index]

    def __len__(self) -> Int:
        return len(self.storage)
