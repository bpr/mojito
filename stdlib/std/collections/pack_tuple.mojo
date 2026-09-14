# Compatibility wrapper for the earlier Mojito prototype.  New tuple displays
# resolve to `std.builtin.tuple.Tuple`; explicit PackTuple imports continue
# to work during the migration.

from std.builtin.tuple import Tuple

struct PackTuple[*Ts: Copyable & Movable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def __getitem__[index: Int](self) -> Self.Ts[index]:
        return self.storage[index]

    def __getitem_param__[index: Int](self) -> Self.Ts[index]:
        return self.storage[index]

    def __len__(self) -> Int:
        return len(self.storage)
